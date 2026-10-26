//! Small helpers.

/// "45 seconds", "3 minutes", "2 hours".
pub fn human_duration(seconds: u64) -> String {
    let (n, unit) = match seconds {
        0..=59 => (seconds.max(1), "second"),
        60..=3599 => (seconds.div_ceil(60), "minute"),
        _ => (seconds.div_ceil(3600), "hour"),
    };
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

/// Trim and collapse a display name; `None` when empty or too long.
pub fn clean_name(raw: &str, max_chars: usize) -> Option<String> {
    let name = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let len = name.chars().count();
    (len > 0 && len <= max_chars && !name.chars().any(char::is_control)).then_some(name)
}

/// Truncate to `max` chars, adding an ellipsis.
pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_names_and_truncation() {
        assert_eq!(human_duration(0), "1 second");
        assert_eq!(human_duration(61), "2 minutes");
        assert_eq!(human_duration(3600), "1 hour");
        assert_eq!(
            clean_name("  Studio   Mac ", 40).as_deref(),
            Some("Studio Mac")
        );
        assert_eq!(clean_name("   ", 40), None);
        assert_eq!(clean_name("a\u{7}b", 40), None);
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
    }
}
