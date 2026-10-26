//! [`Op::Screenshot`](dot_wire::Op::Screenshot) with `screencapture` and `sips`.
use super::Ctx;
use crate::error::{self, OpResult};
use crate::proc::{self, Command};
use crate::script::{self, sources};
use base64::Engine;
use dot_wire::{ImageFormat, Rect, Screenshot, ScreenshotArgs};
use serde::Deserialize;
use std::path::Path;

const SCREENCAPTURE: &str = "/usr/sbin/screencapture";
const SIPS: &str = "/usr/bin/sips";
/// Default and bounds for `max_width`.
const DEFAULT_MAX_WIDTH: u32 = 1440;
const MIN_MAX_WIDTH: u32 = 64;
const MAX_MAX_WIDTH: u32 = 8192;

#[derive(Debug, Deserialize)]
struct ScreenDisplay {
    index: u32,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Debug, Deserialize)]
struct ScreenWindow {
    id: u64,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Debug, Deserialize)]
struct ScreenInfo {
    displays: Vec<ScreenDisplay>,
    screen_recording: Option<bool>,
    window: Option<ScreenWindow>,
}

fn screen_recording_denied() -> dot_wire::GuestError {
    error::permission(
        "macOS blocks screenshots until the agent has Screen Recording permission.",
        error::HINT_SCREEN_RECORDING,
    )
}

/// What to pass to `screencapture`, and the area that covers in points.
fn plan(args: &ScreenshotArgs, info: &ScreenInfo) -> OpResult<(Vec<String>, Rect)> {
    if let Some(window) = &info.window {
        return Ok((
            vec!["-o".into(), "-l".into(), window.id.to_string()],
            Rect {
                x: window.x,
                y: window.y,
                width: window.width,
                height: window.height,
            },
        ));
    }
    if let Some(region) = args.region {
        let finite = [region.x, region.y, region.width, region.height]
            .iter()
            .all(|v| v.is_finite());
        if !finite || region.width < 1.0 || region.height < 1.0 {
            return Err(error::invalid(
                "The region needs a width and height of at least 1 point.",
            ));
        }
        let r = Rect {
            x: region.x.round(),
            y: region.y.round(),
            width: region.width.round(),
            height: region.height.round(),
        };
        let spec = format!("{},{},{},{}", r.x, r.y, r.width, r.height);
        return Ok((vec!["-R".into(), spec], r));
    }
    let index = args.display.unwrap_or(1);
    let display = info
        .displays
        .iter()
        .find(|d| d.index == index)
        .ok_or_else(|| {
            if info.displays.is_empty() {
                error::failed("macOS reports no displays. Is anyone logged in to the Mac's screen?")
            } else {
                error::invalid(format!(
                    "There is no display {index}; this Mac has {}.",
                    info.displays.len()
                ))
            }
        })?;
    Ok((
        vec!["-D".into(), index.to_string()],
        Rect {
            x: display.x,
            y: display.y,
            width: display.width,
            height: display.height,
        },
    ))
}

/// Capture, downscale and encode.
pub async fn capture(ctx: &Ctx, args: ScreenshotArgs) -> OpResult<Screenshot> {
    let max_width = args.max_width.unwrap_or(DEFAULT_MAX_WIDTH);
    if !(MIN_MAX_WIDTH..=MAX_MAX_WIDTH).contains(&max_width) {
        return Err(error::invalid(format!(
            "max_width must be between {MIN_MAX_WIDTH} and {MAX_MAX_WIDTH}."
        )));
    }
    let info: ScreenInfo = script::jxa(
        ctx.deadline,
        sources::SCREEN,
        &serde_json::json!({ "app": args.app }),
    )
    .await?;
    if info.screen_recording == Some(false) {
        return Err(screen_recording_denied());
    }
    let (target, area) = plan(&args, &info)?;

    let dir = crate::util::temp_dir()?;
    let raw = dir.path().join("capture.png");
    let mut cmd = Command::new(SCREENCAPTURE, ctx.deadline).args(["-x", "-t", "png"]);
    if args.cursor {
        cmd = cmd.arg("-C");
    }
    let out = proc::tool(cmd.args(&target).arg(&raw)).await?;
    let captured = std::fs::metadata(&raw).map(|m| m.len()).unwrap_or(0);
    if !out.success() || captured == 0 {
        let stderr = out.stderr_text();
        if stderr.to_lowercase().contains("could not create image") || captured == 0 {
            return Err(screen_recording_denied());
        }
        return Err(error::failed(format!("screencapture failed: {stderr}")));
    }

    let (raw_width, _) = pixel_size(ctx, &raw).await?;
    let (mime, ext, format) = match args.format {
        ImageFormat::Png => ("image/png", "png", "png"),
        ImageFormat::Jpeg => ("image/jpeg", "jpg", "jpeg"),
    };
    let final_path = dir.path().join(format!("final.{ext}"));
    let needs_resize = raw_width > max_width;
    if needs_resize || args.format == ImageFormat::Jpeg {
        let mut cmd = Command::new(SIPS, ctx.deadline).args(["-s", "format", format]);
        if args.format == ImageFormat::Jpeg {
            cmd = cmd.args(["-s", "formatOptions", "80"]);
        }
        if needs_resize {
            cmd = cmd.args(["--resampleWidth", &max_width.to_string()]);
        }
        proc::tool_ok(cmd.arg(&raw).arg("--out").arg(&final_path)).await?;
    } else {
        std::fs::rename(&raw, &final_path)
            .map_err(|e| error::failed(format!("Could not prepare the image: {e}")))?;
    }
    let (width, height) = pixel_size(ctx, &final_path).await?;
    let bytes = tokio::fs::read(&final_path)
        .await
        .map_err(|e| error::failed(format!("Could not read the screenshot: {e}")))?;
    drop(dir);
    Ok(Screenshot {
        data: base64::engine::general_purpose::STANDARD.encode(&bytes),
        mime: mime.to_owned(),
        width,
        height,
        area,
        points_per_pixel: if width == 0 {
            1.0
        } else {
            area.width / f64::from(width)
        },
    })
}

async fn pixel_size(ctx: &Ctx, path: &Path) -> OpResult<(u32, u32)> {
    let out = proc::tool_ok(
        Command::new(SIPS, ctx.deadline)
            .args(["-g", "pixelWidth", "-g", "pixelHeight"])
            .arg(path),
    )
    .await?;
    parse_sips_size(&out.stdout_text())
        .ok_or_else(|| error::failed("Could not read the screenshot's size."))
}

/// `  pixelWidth: 3456` / `  pixelHeight: 2234` from `sips -g`.
fn parse_sips_size(text: &str) -> Option<(u32, u32)> {
    let field = |key: &str| {
        text.lines()
            .find_map(|l| l.trim().strip_prefix(key))
            .and_then(|v| v.trim().parse().ok())
    };
    Some((field("pixelWidth:")?, field("pixelHeight:")?))
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        reason = "Test assertions fail immediately on malformed setup"
    )]
    use super::*;

    fn info() -> ScreenInfo {
        ScreenInfo {
            displays: vec![
                ScreenDisplay {
                    index: 1,
                    x: 0.0,
                    y: 0.0,
                    width: 1728.0,
                    height: 1117.0,
                },
                ScreenDisplay {
                    index: 2,
                    x: 1728.0,
                    y: -200.0,
                    width: 2560.0,
                    height: 1440.0,
                },
            ],
            screen_recording: Some(true),
            window: None,
        }
    }

    #[test]
    fn display_capture_covers_that_display_in_global_points() {
        let args = ScreenshotArgs {
            display: Some(2),
            ..ScreenshotArgs::default()
        };
        let (cli, area) = plan(&args, &info()).unwrap();
        assert_eq!(cli, ["-D", "2"]);
        assert_eq!(
            area,
            Rect {
                x: 1728.0,
                y: -200.0,
                width: 2560.0,
                height: 1440.0
            }
        );
        let args = ScreenshotArgs {
            display: Some(3),
            ..ScreenshotArgs::default()
        };
        assert_eq!(
            plan(&args, &info()).unwrap_err().code,
            dot_wire::GuestErrorCode::InvalidArgs
        );
    }

    #[test]
    fn region_and_window_targets() {
        let args = ScreenshotArgs {
            region: Some(Rect {
                x: 10.4,
                y: 20.0,
                width: 300.0,
                height: 200.6,
            }),
            ..ScreenshotArgs::default()
        };
        let (cli, area) = plan(&args, &info()).unwrap();
        assert_eq!(cli, ["-R", "10,20,300,201"]);
        assert_eq!(area.height, 201.0);
        let bad = ScreenshotArgs {
            region: Some(Rect {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 10.0,
            }),
            ..ScreenshotArgs::default()
        };
        assert!(plan(&bad, &info()).is_err());

        let mut with_window = info();
        with_window.window = Some(ScreenWindow {
            id: 42,
            x: 5.0,
            y: 6.0,
            width: 7.0,
            height: 8.0,
        });
        let (cli, area) = plan(&ScreenshotArgs::default(), &with_window).unwrap();
        assert_eq!(cli, ["-o", "-l", "42"]);
        assert_eq!(area.width, 7.0);
    }

    #[test]
    fn sips_size_output_parses() {
        let text = "/tmp/x.png\n  pixelWidth: 3456\n  pixelHeight: 2234\n";
        assert_eq!(parse_sips_size(text), Some((3456, 2234)));
        assert_eq!(parse_sips_size("/tmp/x.png\n"), None);
    }
}
