//! Building [`GuestError`]s, and the one parser for `osascript` failures.
use dot_wire::{GuestError, GuestErrorCode as Code};

/// What every operation returns.
pub type OpResult<T = serde_json::Value> = Result<T, GuestError>;

/// Where to grant Accessibility.
pub const HINT_ACCESSIBILITY: &str = "Open System Settings > Privacy & Security > Accessibility and turn on dot-guest, then try again.";
/// Where to grant Screen Recording.
pub const HINT_SCREEN_RECORDING: &str = "Open System Settings > Privacy & Security > Screen & System Audio Recording, turn on dot-guest, then restart the agent (log out and back in, or run `dot-guest install` again).";
/// Where to grant Full Disk Access.
pub const HINT_FULL_DISK: &str =
    "Open System Settings > Privacy & Security > Full Disk Access and turn on dot-guest.";
/// How to let Safari run JavaScript sent by the agent.
pub const HINT_SAFARI_JS: &str = "In Safari, open Settings > Advanced and turn on \"Show features for web developers\", then choose Develop > Allow JavaScript from Apple Events.";

/// Arguments were wrong.
pub fn invalid(message: impl Into<String>) -> GuestError {
    GuestError::new(Code::InvalidArgs, message)
}

/// Something does not exist.
pub fn not_found(message: impl Into<String>) -> GuestError {
    GuestError::new(Code::NotFound, message)
}

/// Something ran and failed.
pub fn failed(message: impl Into<String>) -> GuestError {
    GuestError::new(Code::Failed, message)
}

/// The Mac lacks a tool or feature.
pub fn unsupported(message: impl Into<String>) -> GuestError {
    GuestError::new(Code::Unsupported, message)
}

/// A privacy setting blocks this.
pub fn permission(message: impl Into<String>, hint: impl Into<String>) -> GuestError {
    GuestError::new(Code::PermissionDenied, message).hint(hint)
}

/// The deadline passed.
pub fn timeout(message: impl Into<String>) -> GuestError {
    GuestError::new(Code::Timeout, message)
}

/// Where to allow the agent to control `app` with Apple events.
pub fn automation_hint(app: Option<&str>) -> String {
    match app {
        Some(app) => format!(
            "Open System Settings > Privacy & Security > Automation, find dot-guest and turn on {app}."
        ),
        None => "Open System Settings > Privacy & Security > Automation, find dot-guest and turn on the app it needs to control.".to_owned(),
    }
}

/// Cap a message so a runaway stderr can't flood a reply.
fn cap(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

/// Turn the stderr of a failed `osascript` run into a typed error.
///
/// Handles both AppleScript (`file:12:25: execution error: Can't get window 1. (-1719)`)
/// and JXA (`execution error: Error: Error: NOT_FOUND: no such tab (-2700)`). Bundled
/// scripts signal typed failures by throwing `Error("CODE: message")`.
pub fn osascript_error(stderr: &str) -> GuestError {
    let text = stderr.trim();
    let (syntax, body) = locate_message(text);
    let (body, number) = split_error_number(body);
    let mut body = body.trim();
    while let Some(rest) = body.strip_prefix("Error: ") {
        body = rest.trim_start();
    }
    let body = cap(body, 2000);

    if let Some((code, message)) = typed_prefix(&body) {
        return GuestError::new(code, message);
    }
    let lower = body.to_lowercase();
    if lower.contains("assistive access") || number == Some(-25211) {
        return permission(
            "macOS blocked the agent from reading or controlling other apps (Accessibility is off).",
            HINT_ACCESSIBILITY,
        );
    }
    if number == Some(-1743) || lower.contains("not authorized to send apple events") {
        let app = body
            .split_once("Apple events to ")
            .map(|(_, rest)| rest.trim_end_matches('.').trim());
        return permission(
            format!(
                "macOS blocked the agent from controlling {} (Automation is off).",
                app.unwrap_or("that app")
            ),
            automation_hint(app),
        );
    }
    if lower.contains("allow javascript from apple events") {
        return permission(
            "Safari is not allowing JavaScript from Apple Events, so the agent can't read or act on pages.",
            HINT_SAFARI_JS,
        );
    }
    if number == Some(-10004) || lower.contains("privilege violation") {
        return permission(
            format!("macOS refused: {body}"),
            "Check System Settings > Privacy & Security for the permission this app needs.",
        );
    }
    if number == Some(-1712) || lower.contains("timed out") {
        return timeout(format!("The app did not answer in time: {body}"));
    }
    if number == Some(-128) {
        return failed("The action was cancelled in the Mac (someone clicked Cancel).");
    }
    if syntax || matches!(number, Some(-2741 | -2740 | -2750 | -2753)) {
        return invalid(format!("The script has a syntax error: {body}"));
    }
    if matches!(number, Some(-1728 | -1719 | -10814 | -600 | -609))
        || lower.contains("can't be found")
        || lower.contains("can\u{2019}t be found")
        || lower.contains("isn't running")
        || lower.contains("isn\u{2019}t running")
    {
        return not_found(body);
    }
    if body.is_empty() {
        return failed(format!("The script failed: {}", cap(text, 2000)));
    }
    failed(body)
}

/// Find the human part of an osascript error line. Returns whether it was a syntax error.
fn locate_message(text: &str) -> (bool, &str) {
    let markers = [
        ("execution error: ", false),
        ("script error: ", true),
        ("syntax error: ", true),
    ];
    let found = markers
        .iter()
        .filter_map(|(marker, syntax)| text.find(marker).map(|at| (at, marker.len(), *syntax)))
        .min_by_key(|(at, _, _)| *at);
    match found {
        Some((at, len, syntax)) => (syntax, &text[at + len..]),
        None => (false, text),
    }
}

/// Split `message (-1728)` into the message and the number.
fn split_error_number(body: &str) -> (&str, Option<i32>) {
    let trimmed = body.trim_end();
    if let Some(inner) = trimmed.strip_suffix(')')
        && let Some(open) = inner.rfind('(')
        && let Ok(number) = inner[open + 1..].trim().parse::<i32>()
    {
        return (&trimmed[..open], Some(number));
    }
    (trimmed, None)
}

/// `NOT_FOUND: message` thrown by a bundled script.
fn typed_prefix(body: &str) -> Option<(Code, String)> {
    let (prefix, rest) = body.split_once(": ")?;
    let code = match prefix {
        "INVALID_ARGS" => Code::InvalidArgs,
        "NOT_FOUND" => Code::NotFound,
        "PERMISSION_DENIED" => Code::PermissionDenied,
        "FAILED" | "CONFLICT" => Code::Failed,
        "TIMEOUT" => Code::Timeout,
        "STALE_REF" => Code::StaleRef,
        "TOO_LARGE" => Code::TooLarge,
        "UNSUPPORTED" => Code::Unsupported,
        _ => return None,
    };
    Some((code, rest.trim().to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(stderr: &str) -> GuestError {
        osascript_error(stderr)
    }

    #[test]
    fn typed_errors_thrown_by_jxa_keep_their_code_and_message() {
        let err = parse("execution error: Error: Error: NOT_FOUND: no such thing (-2700)");
        assert_eq!(err, not_found("no such thing"));
        let err = parse("t.js: execution error: Error: Error: INVALID_ARGS: bad \"q\" (-2700)\n");
        assert_eq!(err, invalid("bad \"q\""));
        let err = parse("execution error: Error: Error: CONFLICT: tab moved (-2700)");
        assert_eq!(err.code, Code::Failed);
        let err = parse("execution error: Error: Error: STALE_REF: the window closed (-2700)");
        assert_eq!(err.code, Code::StaleRef);
    }

    #[test]
    fn applescript_missing_objects_are_not_found_without_the_file_prefix() {
        let err = parse(
            "/var/folders/x/t.applescript:18:36: execution error: Can\u{2019}t get thing. (-1728)",
        );
        assert_eq!(err, not_found("Can\u{2019}t get thing."));
        let err = parse("execution error: Error: Error: Application can't be found. (-2700)");
        assert_eq!(err.code, Code::NotFound);
    }

    #[test]
    fn syntax_errors_in_either_language_are_invalid_args() {
        let err = parse(
            "s.applescript:23:24: script error: Expected expression but found end of line. (-2741)",
        );
        assert_eq!(err.code, Code::InvalidArgs);
        assert!(err.message.contains("Expected expression"));
        let err = parse(
            "0:16: syntax error: Error on line 1: SyntaxError: Unexpected end of script (-2700)",
        );
        assert_eq!(err.code, Code::InvalidArgs);
    }

    #[test]
    fn automation_denial_names_the_app_in_the_hint() {
        let err =
            parse("execution error: Not authorized to send Apple events to System Events. (-1743)");
        assert_eq!(err.code, Code::PermissionDenied);
        let hint = err.hint.unwrap_or_default();
        assert!(
            hint.contains("Automation") && hint.contains("System Events"),
            "{hint}"
        );
    }

    #[test]
    fn assistive_access_is_accessibility_even_with_an_invalid_index_number() {
        let err = parse(
            "execution error: System Events got an error: osascript is not allowed assistive access. (-1719)",
        );
        assert_eq!(err.code, Code::PermissionDenied);
        assert_eq!(err.hint.as_deref(), Some(HINT_ACCESSIBILITY));
    }

    #[test]
    fn safari_javascript_switch_maps_to_permission_with_the_menu_path() {
        let err = parse(
            "execution error: Error: Error: You must enable the 'Allow JavaScript from Apple Events' option in Safari's Develop menu to use 'do JavaScript'. (-2700)",
        );
        assert_eq!(err.code, Code::PermissionDenied);
        assert_eq!(err.hint.as_deref(), Some(HINT_SAFARI_JS));
    }

    #[test]
    fn timeouts_and_plain_failures() {
        assert_eq!(
            parse("execution error: Safari got an error: AppleEvent timed out. (-1712)").code,
            Code::Timeout
        );
        let err = parse(
            "execution error: Error: TypeError: null is not an object (evaluating 'null.x') (-2700)",
        );
        assert_eq!(
            err,
            failed("TypeError: null is not an object (evaluating 'null.x')")
        );
        assert_eq!(parse("").code, Code::Failed);
    }
}
