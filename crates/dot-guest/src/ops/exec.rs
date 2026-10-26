//! [`Op::Exec`](dot_wire::Op::Exec): a shell command as the signed-in user.
use super::Ctx;
use crate::error::{self, OpResult};
use crate::proc::{self, Command};
use dot_wire::{Exec, ExecResult};
use std::time::Duration;

/// Largest output cap a caller may ask for.
pub const MAX_OUTPUT_CAP: usize = 32 * 1024 * 1024;
/// Time kept back from the call's deadline to report a timeout instead of being cut off.
const REPORT_MARGIN: Duration = Duration::from_millis(150);

/// Run `/bin/zsh -lc <command>`. Hitting the deadline is a normal result with
/// `timed_out` set (and the process group killed), so partial output is not lost.
pub async fn exec(ctx: &Ctx, args: Exec) -> OpResult<ExecResult> {
    if args.command.trim().is_empty() {
        return Err(error::invalid("The command is empty."));
    }
    if args.max_output_bytes > MAX_OUTPUT_CAP {
        return Err(error::invalid(format!(
            "max_output_bytes can be at most {MAX_OUTPUT_CAP}."
        )));
    }
    // 0 means "not set" (an older host's derived Default); use the protocol default.
    let max_output = match args.max_output_bytes {
        0 => Exec::default().max_output_bytes,
        n => n,
    };
    let home = &ctx.state.home;
    let cwd = match args.cwd.as_deref() {
        Some(raw) => crate::util::expand_path(raw, home)?,
        None => home.clone(),
    };
    if !cwd.is_dir() {
        return Err(error::not_found(format!(
            "The working folder {} does not exist.",
            cwd.display()
        )));
    }
    for key in args.env.keys() {
        if key.is_empty() || key.contains('=') || key.contains('\0') {
            return Err(error::invalid(format!(
                "`{key}` is not a valid environment variable name."
            )));
        }
    }
    let deadline = ctx
        .deadline
        .checked_sub(REPORT_MARGIN)
        .unwrap_or(ctx.deadline);
    let mut cmd = Command::new("/bin/zsh", deadline)
        .args(["-lc", &args.command])
        .cwd(cwd)
        .max_output(max_output);
    for (key, value) in &args.env {
        cmd = cmd.env(key, value);
    }
    if let Some(stdin) = args.stdin {
        cmd = cmd.stdin(stdin.into_bytes());
    }
    let out = cmd
        .run()
        .await
        .map_err(|e| proc::spawn_error("/bin/zsh", &e))?;
    Ok(ExecResult {
        exit_code: out.code,
        stdout: out.stdout_text(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        truncated: out.truncated,
        timed_out: out.timed_out,
        duration_ms: u64::try_from(out.duration.as_millis()).unwrap_or(u64::MAX),
    })
}
