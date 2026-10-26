//! Hiding credentials in what the guest shows the host: a server's command line, its
//! stderr, and HTTP error bodies.

/// What a hidden secret looks like.
pub const HIDDEN: &str = "\u{2022}\u{2022}\u{2022}";

/// Whether an option, variable or header name suggests its value is a credential.
fn secret_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    ["key", "token", "secret", "password", "auth", "bearer"]
        .iter()
        .any(|word| name.contains(word))
}

/// The arguments with anything that looks like a credential replaced by [`HIDDEN`].
pub fn redact_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut hide_next = false;
    for arg in args {
        if std::mem::take(&mut hide_next) && !arg.starts_with("--") {
            out.push(HIDDEN.to_owned());
            continue;
        }
        if let Some(flag) = arg.strip_prefix('-') {
            match arg.split_once('=') {
                Some((name, _)) if secret_name(name) => out.push(format!("{name}={HIDDEN}")),
                Some((name, value)) => out.push(format!("{name}={}", redact_value(value))),
                None => {
                    hide_next = secret_name(flag);
                    out.push(arg.clone());
                }
            }
        } else if arg.eq_ignore_ascii_case("bearer") {
            hide_next = true;
            out.push(arg.clone());
        } else {
            out.push(redact_value(arg));
        }
    }
    out
}

/// One value: `NAME=secret`, `Authorization: ...`, `Bearer ...`, or a bare credential.
fn redact_value(value: &str) -> String {
    if value.contains("://") {
        return redact_url(value);
    }
    if let Some((name, _)) = value.split_once('=')
        && is_var_name(name)
        && secret_name(name)
    {
        return format!("{name}={HIDDEN}");
    }
    if let Some((name, _)) = value.split_once(':')
        && !name.is_empty()
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && secret_name(name)
    {
        return format!("{name}: {HIDDEN}");
    }
    if let Some(at) = value.to_ascii_lowercase().find("bearer ") {
        return format!("{}{HIDDEN}", &value[..at + "bearer ".len()]);
    }
    if looks_like_credential(value) {
        return HIDDEN.to_owned();
    }
    value.to_owned()
}

fn is_var_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Well-known key prefixes, or a long run of hex, or of mixed-case base64 with digits.
/// Ordinary arguments (package names, paths, flags) are lowercase words or have `/`.
fn looks_like_credential(value: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "sk-",
        "sk_",
        "ghp_",
        "gho_",
        "ghs_",
        "ghu_",
        "github_pat_",
        "xox",
        "AKIA",
        "eyJ",
    ];
    if value.contains([' ', '/']) {
        return false;
    }
    if value.len() >= 12 && PREFIXES.iter().any(|p| value.starts_with(p)) {
        return true;
    }
    if value.len() < 32 {
        return false;
    }
    let hex_digits = value.bytes().filter(u8::is_ascii_hexdigit).count();
    let hex = hex_digits >= 32 && value.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-');
    let base64 = value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"+=_-".contains(&b))
        && value.bytes().any(|b| b.is_ascii_digit())
        && value.bytes().any(|b| b.is_ascii_uppercase())
        && value.bytes().any(|b| b.is_ascii_lowercase());
    hex || base64
}

/// A URL with its password and secret-looking query values hidden; the rest stays
/// readable (`postgresql://me:•••@db.local/app`).
pub fn redact_url(url: &str) -> String {
    let (base, query) = match url.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (url, None),
    };
    let mut out = match base.find("://") {
        Some(at) => {
            let (scheme, rest) = base.split_at(at + 3);
            let end = rest.find('/').unwrap_or(rest.len());
            let (authority, path) = rest.split_at(end);
            match authority.rsplit_once('@') {
                Some((userinfo, host)) => {
                    let user = match userinfo.split_once(':') {
                        Some((user, _)) => format!("{user}:{HIDDEN}"),
                        None if looks_like_credential(userinfo) => HIDDEN.to_owned(),
                        None => userinfo.to_owned(),
                    };
                    format!("{scheme}{user}@{host}{path}")
                }
                None => base.to_owned(),
            }
        }
        None => base.to_owned(),
    };
    if let Some(query) = query {
        let query = query
            .split('&')
            .map(|pair| match pair.split_once('=') {
                Some((name, _)) if secret_name(name) => format!("{name}={HIDDEN}"),
                Some((name, value)) if looks_like_credential(value) => {
                    format!("{name}={HIDDEN}")
                }
                _ => pair.to_owned(),
            })
            .collect::<Vec<_>>()
            .join("&");
        out.push('?');
        out.push_str(&query);
    }
    out
}

/// Text from a server (stderr, an error page) made safe to show: each value in
/// `secrets` of 6 or more characters (the launch config's environment) and every word
/// that looks like a credential is hidden, then only the last `keep` characters are kept.
pub fn redact_text(text: &str, secrets: &[String], keep: usize) -> String {
    let mut secrets: Vec<&str> = secrets
        .iter()
        .map(String::as_str)
        .filter(|s| s.chars().count() >= 6)
        .collect();
    // Longest first, so a secret that contains another is hidden whole.
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let mut text = text.to_owned();
    for secret in secrets {
        text = text.replace(secret, HIDDEN);
    }
    let words: Vec<String> = text.split_whitespace().map(ToOwned::to_owned).collect();
    let text = redact_args(&words).join(" ");
    match text.char_indices().rev().nth(keep) {
        Some((at, c)) => format!("...{}", &text[at + c.len_utf8()..]),
        None => text,
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_are_hidden_from_the_detail() {
        let h = HIDDEN;
        let cases: &[(&[&str], String)] = &[
            (
                &["--api-key", "abc", "--port", "80"],
                format!("--api-key {h} --port 80"),
            ),
            (
                &["--auth-token=abc", "--name=x"],
                format!("--auth-token={h} --name=x"),
            ),
            (&["--token", "--verbose"], "--token --verbose".to_owned()),
            (
                &["GITHUB_TOKEN=abc", "MODE=fast"],
                format!("GITHUB_TOKEN={h} MODE=fast"),
            ),
            (
                &["-H", "Authorization: Bearer abc"],
                format!("-H Authorization: {h}"),
            ),
            (
                &["--header=X-Api-Key: abc"],
                format!("--header=X-Api-Key: {h}"),
            ),
            (&["Bearer", "abc"], format!("Bearer {h}")),
            (&["token is Bearer abc"], format!("token is Bearer {h}")),
            (
                &["sk-proj-abcdef123456", "ghp_abcdefghij12", "xoxb-1234-5678"],
                format!("{h} {h} {h}"),
            ),
            (
                &["AKIAABCDEFGHIJKLMNOP", "github_pat_11AAAAAAA"],
                format!("{h} {h}"),
            ),
            (&["0123456789abcdef0123456789abcdef"], h.to_owned()),
            (&["aB3dE6gH9jK2mN5pQ8sT1vW4yZ7bC0eF"], h.to_owned()),
            // Ordinary arguments stay readable.
            (
                &[
                    "-y",
                    "@modelcontextprotocol/server-filesystem",
                    "/Users/me/Documents",
                ],
                "-y @modelcontextprotocol/server-filesystem /Users/me/Documents".to_owned(),
            ),
            (
                &["some-long-mcp-server-package-name-v2"],
                "some-long-mcp-server-package-name-v2".to_owned(),
            ),
            (&["--keep-alive", "30"], "--keep-alive 30".to_owned()),
        ];
        for (args, want) in cases {
            let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
            assert_eq!(redact_args(&args).join(" "), *want, "{args:?}");
        }
        let urls = [
            (
                "http://127.0.0.1:9000/mcp?token=abc&mode=x",
                format!("http://127.0.0.1:9000/mcp?token={h}&mode=x"),
            ),
            (
                "postgresql://me:hunter2@db.local:5432/app?sslmode=require",
                format!("postgresql://me:{h}@db.local:5432/app?sslmode=require"),
            ),
            (
                "https://ghp_abcdefghij12@github.com/me/repo",
                format!("https://{h}@github.com/me/repo"),
            ),
            (
                "https://example.com/a/b?q=1",
                "https://example.com/a/b?q=1".to_owned(),
            ),
        ];
        for (url, want) in urls {
            assert_eq!(redact_url(url), want);
            assert_eq!(redact_args(&[url.to_owned()]), std::slice::from_ref(&want));
        }
    }

    #[test]
    fn server_text_hides_config_values_and_credential_words_and_keeps_the_end() {
        let secrets = ["hunter2-long".to_owned(), "short".to_owned()];
        let text = "boot\nError: login failed for hunter2-long (short)\n  using key sk-proj-abcdef123456 --password p4ss\n";
        let h = HIDDEN;
        assert_eq!(
            redact_text(text, &secrets, 300),
            format!("boot Error: login failed for {h} (short) using key {h} --password {h}")
        );
        assert_eq!(redact_text("aaaa bbbb cccc", &[], 9), "...bbbb cccc");
    }
}
