//! Mouse and keyboard input, posted as CoreGraphics events by `scripts/input.js`.
use super::Ctx;
use super::misc::{OPEN, open_app_args};
use crate::error::{self, OpResult};
use crate::proc::{self, Command};
use crate::script::{self, sources};
use dot_wire::{Button, KeyboardArgs, PointerArgs};
use serde::Serialize;
use serde_json::{Value, json};

/// Longest text typed in one call.
const MAX_TEXT_CHARS: usize = 100_000;

const SHIFT: u64 = 0x2_0000;
const CONTROL: u64 = 0x4_0000;
const OPTION: u64 = 0x8_0000;
const COMMAND: u64 = 0x10_0000;
const NUMERIC_PAD: u64 = 0x20_0000;
const SECONDARY_FN: u64 = 0x80_0000;

/// One key press with modifiers held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct KeyPress {
    /// macOS virtual key code (ANSI layout).
    pub code: u16,
    /// `CGEventFlags`.
    pub flags: u64,
}

fn modifier(name: &str) -> Option<u64> {
    Some(match name {
        "cmd" | "command" | "meta" | "super" | "\u{2318}" => COMMAND,
        "shift" | "\u{21e7}" => SHIFT,
        "option" | "opt" | "alt" | "\u{2325}" => OPTION,
        "ctrl" | "control" | "\u{2303}" => CONTROL,
        "fn" => SECONDARY_FN,
        _ => return None,
    })
}

/// Parse `cmd`, `shift`, ... into event flags.
pub fn modifier_flags(names: &[String]) -> Result<u64, String> {
    names.iter().try_fold(0, |flags, name| {
        modifier(&name.trim().to_lowercase())
            .map(|f| flags | f)
            .ok_or_else(|| {
                format!("`{name}` is not a modifier. Use cmd, shift, option, ctrl or fn.")
            })
    })
}

/// Key code for an unshifted character on the ANSI layout.
fn char_code(c: char) -> Option<u16> {
    Some(match c {
        'a' => 0,
        's' => 1,
        'd' => 2,
        'f' => 3,
        'h' => 4,
        'g' => 5,
        'z' => 6,
        'x' => 7,
        'c' => 8,
        'v' => 9,
        'b' => 11,
        'q' => 12,
        'w' => 13,
        'e' => 14,
        'r' => 15,
        'y' => 16,
        't' => 17,
        '1' => 18,
        '2' => 19,
        '3' => 20,
        '4' => 21,
        '6' => 22,
        '5' => 23,
        '=' => 24,
        '9' => 25,
        '7' => 26,
        '-' => 27,
        '8' => 28,
        '0' => 29,
        ']' => 30,
        'o' => 31,
        'u' => 32,
        '[' => 33,
        'i' => 34,
        'p' => 35,
        'l' => 37,
        'j' => 38,
        '\'' => 39,
        'k' => 40,
        ';' => 41,
        '\\' => 42,
        ',' => 43,
        '/' => 44,
        'n' => 45,
        'm' => 46,
        '.' => 47,
        '`' => 50,
        _ => return None,
    })
}

/// The unshifted key that types a shifted symbol on the ANSI layout.
fn shifted_base(c: char) -> Option<char> {
    Some(match c {
        '!' => '1',
        '@' => '2',
        '#' => '3',
        '$' => '4',
        '%' => '5',
        '^' => '6',
        '&' => '7',
        '*' => '8',
        '(' => '9',
        ')' => '0',
        '_' => '-',
        '+' => '=',
        '{' => '[',
        '}' => ']',
        '|' => '\\',
        ':' => ';',
        '"' => '\'',
        '<' => ',',
        '>' => '.',
        '?' => '/',
        '~' => '`',
        _ => return None,
    })
}

/// A key name or single character to (code, implied flags).
fn key(name: &str) -> Option<(u16, u64)> {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if let Some(code) = char_code(c) {
            return Some((code, 0));
        }
        if c.is_ascii_uppercase() {
            return char_code(c.to_ascii_lowercase()).map(|code| (code, SHIFT));
        }
        return shifted_base(c)
            .and_then(char_code)
            .map(|code| (code, SHIFT));
    }
    let lower = name.to_lowercase();
    let arrow = NUMERIC_PAD | SECONDARY_FN;
    let named = match lower.as_str() {
        "return" | "enter" => (36, 0),
        "tab" => (48, 0),
        "space" | "spacebar" => (49, 0),
        "delete" | "backspace" => (51, 0),
        "escape" | "esc" => (53, 0),
        "forwarddelete" | "fwddelete" | "del" => (117, SECONDARY_FN),
        "home" => (115, SECONDARY_FN),
        "end" => (119, SECONDARY_FN),
        "pageup" => (116, SECONDARY_FN),
        "pagedown" => (121, SECONDARY_FN),
        "help" => (114, SECONDARY_FN),
        "left" | "arrowleft" => (123, arrow),
        "right" | "arrowright" => (124, arrow),
        "down" | "arrowdown" => (125, arrow),
        "up" | "arrowup" => (126, arrow),
        "minus" => (27, 0),
        "equal" | "equals" => (24, 0),
        "plus" => (24, SHIFT),
        "comma" => (43, 0),
        "period" | "dot" => (47, 0),
        "slash" => (44, 0),
        "backslash" => (42, 0),
        "semicolon" => (41, 0),
        "quote" | "apostrophe" => (39, 0),
        "backtick" | "grave" => (50, 0),
        "leftbracket" => (33, 0),
        "rightbracket" => (30, 0),
        _ => return function_key(&lower).map(|code| (code, SECONDARY_FN)),
    };
    Some(named)
}

fn function_key(name: &str) -> Option<u16> {
    let n: u8 = name.strip_prefix('f')?.parse().ok()?;
    const CODES: [u16; 20] = [
        122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111, 105, 107, 113, 106, 64, 79, 80, 90,
    ];
    CODES.get(usize::from(n).checked_sub(1)?).copied()
}

/// Parse a chord sequence like `cmd+shift+t`, `return` or `cmd+a delete`.
pub fn parse_keys(spec: &str) -> Result<Vec<KeyPress>, String> {
    let chords: Vec<&str> = spec.split_whitespace().collect();
    if chords.is_empty() {
        return Err("No keys given.".to_owned());
    }
    chords.into_iter().map(parse_chord).collect()
}

fn parse_chord(chord: &str) -> Result<KeyPress, String> {
    let (mods, name) = if chord == "+" {
        ("", "+")
    } else if let Some(prefix) = chord.strip_suffix("++") {
        (prefix.strip_suffix('+').unwrap_or(prefix), "+")
    } else {
        chord.rsplit_once('+').unwrap_or(("", chord))
    };
    let mut flags = 0;
    if !mods.is_empty() {
        for m in mods.split('+') {
            flags |= modifier(&m.to_lowercase()).ok_or_else(|| {
                format!("`{m}` in `{chord}` is not a modifier. Use cmd, shift, option, ctrl or fn.")
            })?;
        }
    }
    let (code, implied) = key(name).ok_or_else(|| {
        format!("`{name}` is not a key I know. Use a letter, digit, punctuation, or a name like return, tab, escape, delete, up, f5.")
    })?;
    Ok(KeyPress {
        code,
        flags: flags | implied,
    })
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Step<'a> {
    WaitFront {
        app: &'a str,
    },
    Move {
        x: f64,
        y: f64,
    },
    Click {
        x: f64,
        y: f64,
        button: Button,
        count: u32,
        flags: u64,
    },
    Drag {
        from_x: f64,
        from_y: f64,
        to_x: f64,
        to_y: f64,
    },
    Scroll {
        x: f64,
        y: f64,
        dx: i32,
        dy: i32,
    },
    Text {
        text: &'a str,
    },
    Key {
        code: u16,
        flags: u64,
    },
}

async fn post(ctx: &Ctx, steps: &[Step<'_>]) -> OpResult {
    let _: Value = script::jxa(ctx.deadline, sources::INPUT, &json!({ "steps": steps }))
        .await
        .map_err(|mut e| {
            if e.code == dot_wire::GuestErrorCode::PermissionDenied && e.hint.is_none() {
                e.hint = Some(error::HINT_ACCESSIBILITY.to_owned());
            }
            e
        })?;
    Ok(json!({}))
}

fn point(x: f64, y: f64) -> OpResult<()> {
    if x.is_finite() && y.is_finite() && x.abs() < 100_000.0 && y.abs() < 100_000.0 {
        Ok(())
    } else {
        Err(error::invalid("Coordinates must be finite screen points."))
    }
}

/// Post pointer events without taking the input lock (the caller holds it).
pub async fn post_pointer(ctx: &Ctx, args: &PointerArgs) -> OpResult {
    let step = match args {
        PointerArgs::Click {
            x,
            y,
            button,
            count,
            modifiers,
        } => {
            point(*x, *y)?;
            if !(1..=3).contains(count) {
                return Err(error::invalid("count must be 1, 2 or 3."));
            }
            let flags = modifier_flags(modifiers).map_err(error::invalid)?;
            Step::Click {
                x: *x,
                y: *y,
                button: *button,
                count: *count,
                flags,
            }
        }
        PointerArgs::Move { x, y } => {
            point(*x, *y)?;
            Step::Move { x: *x, y: *y }
        }
        PointerArgs::Drag {
            from_x,
            from_y,
            to_x,
            to_y,
        } => {
            point(*from_x, *from_y)?;
            point(*to_x, *to_y)?;
            Step::Drag {
                from_x: *from_x,
                from_y: *from_y,
                to_x: *to_x,
                to_y: *to_y,
            }
        }
        PointerArgs::Scroll { x, y, dx, dy } => {
            point(*x, *y)?;
            if dx.abs() > 200 || dy.abs() > 200 {
                return Err(error::invalid("Scroll at most 200 lines at a time."));
            }
            if *dx == 0 && *dy == 0 {
                return Err(error::invalid("Give dx or dy lines to scroll."));
            }
            Step::Scroll {
                x: *x,
                y: *y,
                dx: *dx,
                dy: *dy,
            }
        }
    };
    post(ctx, &[step]).await
}

/// [`dot_wire::Op::Pointer`].
pub async fn pointer(ctx: &Ctx, args: PointerArgs) -> OpResult {
    let _input = ctx.state.input_lock.lock().await;
    post_pointer(ctx, &args).await
}

/// Bring an app to the front with LaunchServices, which works from a background agent
/// where `NSRunningApplication.activate` is refused.
pub async fn bring_to_front(ctx: &Ctx, app: &str) -> OpResult<()> {
    let out = proc::tool(Command::new(OPEN, ctx.deadline).args(open_app_args(app))).await?;
    if !out.success() {
        return Err(super::misc::open_error(&out.stderr_text(), Some(app)));
    }
    Ok(())
}

/// The name `wait_front` should match for an app given as a name, bundle id or path.
fn front_name(app: &str) -> &str {
    if app.starts_with('/') {
        let file = app.trim_end_matches('/').rsplit('/').next().unwrap_or(app);
        file.strip_suffix(".app").unwrap_or(file)
    } else {
        app
    }
}

/// [`dot_wire::Op::Keyboard`].
pub async fn keyboard(ctx: &Ctx, args: KeyboardArgs) -> OpResult {
    let text = args.text.as_deref().filter(|t| !t.is_empty());
    let keys = match args
        .keys
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
    {
        Some(spec) => parse_keys(spec).map_err(error::invalid)?,
        None => Vec::new(),
    };
    if text.is_none() && keys.is_empty() {
        return Err(error::invalid(
            "Give `text` to type, `keys` to press, or both.",
        ));
    }
    if let Some(text) = text
        && text.chars().count() > MAX_TEXT_CHARS
    {
        return Err(error::invalid(format!(
            "Type at most {MAX_TEXT_CHARS} characters at a time."
        )));
    }
    let _input = ctx.state.input_lock.lock().await;
    let mut steps = Vec::with_capacity(keys.len() + 2);
    if let Some(app) = args.app.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
        bring_to_front(ctx, app).await?;
        steps.push(Step::WaitFront {
            app: front_name(app),
        });
    }
    if let Some(text) = text {
        steps.push(Step::Text { text });
    }
    steps.extend(keys.iter().map(|k| Step::Key {
        code: k.code,
        flags: k.flags,
    }));
    post(ctx, &steps).await
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        reason = "Test assertions fail immediately on malformed setup"
    )]
    use super::*;

    fn one(spec: &str) -> KeyPress {
        let keys = parse_keys(spec).unwrap();
        assert_eq!(keys.len(), 1, "{spec}");
        keys[0]
    }

    #[test]
    fn chords_combine_modifiers_with_key_codes() {
        assert_eq!(
            one("cmd+shift+t"),
            KeyPress {
                code: 17,
                flags: COMMAND | SHIFT
            }
        );
        assert_eq!(
            one("Cmd+Option+Esc"),
            KeyPress {
                code: 53,
                flags: COMMAND | OPTION
            }
        );
        assert_eq!(
            one("ctrl+a"),
            KeyPress {
                code: 0,
                flags: CONTROL
            }
        );
        assert_eq!(one("return"), KeyPress { code: 36, flags: 0 });
        assert_eq!(one("space"), KeyPress { code: 49, flags: 0 });
        assert_eq!(one("delete"), KeyPress { code: 51, flags: 0 });
        assert_eq!(one("forwarddelete").code, 117);
    }

    #[test]
    fn navigation_and_function_keys_carry_hardware_flags() {
        assert_eq!(
            one("down"),
            KeyPress {
                code: 125,
                flags: NUMERIC_PAD | SECONDARY_FN
            }
        );
        assert_eq!(one("cmd+left").flags, COMMAND | NUMERIC_PAD | SECONDARY_FN);
        assert_eq!(
            one("pagedown"),
            KeyPress {
                code: 121,
                flags: SECONDARY_FN
            }
        );
        assert_eq!(one("f1").code, 122);
        assert_eq!(one("f5").code, 96);
        assert_eq!(one("f20").code, 90);
        assert!(parse_keys("f21").is_err());
        assert!(parse_keys("f0").is_err());
    }

    #[test]
    fn punctuation_and_shifted_symbols() {
        assert_eq!(
            one("cmd+,"),
            KeyPress {
                code: 43,
                flags: COMMAND
            }
        );
        assert_eq!(
            one("cmd++"),
            KeyPress {
                code: 24,
                flags: COMMAND | SHIFT
            }
        );
        assert_eq!(
            one("+"),
            KeyPress {
                code: 24,
                flags: SHIFT
            }
        );
        assert_eq!(
            one("cmd+?"),
            KeyPress {
                code: 44,
                flags: COMMAND | SHIFT
            }
        );
        assert_eq!(
            one("A"),
            KeyPress {
                code: 0,
                flags: SHIFT
            }
        );
        assert_eq!(
            one("cmd+/"),
            KeyPress {
                code: 44,
                flags: COMMAND
            }
        );
        assert_eq!(
            one("cmd+plus"),
            KeyPress {
                code: 24,
                flags: COMMAND | SHIFT
            }
        );
    }

    #[test]
    fn sequences_are_space_separated() {
        let keys = parse_keys("cmd+a  delete").unwrap();
        assert_eq!(
            keys,
            [
                KeyPress {
                    code: 0,
                    flags: COMMAND
                },
                KeyPress { code: 51, flags: 0 }
            ]
        );
    }

    #[test]
    fn unknown_keys_and_modifiers_are_explained() {
        let err = parse_keys("hyper+a").unwrap_err();
        assert!(err.contains("hyper"), "{err}");
        let err = parse_keys("cmd+launch").unwrap_err();
        assert!(err.contains("launch"), "{err}");
        assert!(parse_keys("cmd+").is_err());
        assert!(parse_keys("   ").is_err());
        assert!(parse_keys("\u{e9}").is_err());
    }

    #[test]
    fn modifier_names_for_clicks() {
        let flags = modifier_flags(&["cmd".into(), "Shift".into()]).unwrap();
        assert_eq!(flags, COMMAND | SHIFT);
        assert!(modifier_flags(&["meta2".into()]).is_err());
    }
}
