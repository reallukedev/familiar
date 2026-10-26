//! App packs: typed tools backed by JXA/AppleScript (docs/home/PACKS.md).
use rust_embed::RustEmbed;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;

#[derive(RustEmbed)]
#[folder = "packs/"]
#[include = "*.toml"]
struct Builtin;

/// How risky a tool is; decides defaults and annotations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// Only reads.
    Read,
    /// Changes something in the Mac.
    Write,
    /// Sends something to other people.
    Send,
    /// Changes the Mac itself (install, restore, restart).
    System,
}

/// Script language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    /// JXA.
    Javascript,
    /// AppleScript.
    Applescript,
}

/// A pack manifest.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pack {
    /// `[a-z][a-z0-9_]{1,23}`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// The app it drives.
    #[serde(default)]
    pub bundle_id: Option<String>,
    /// What it's for.
    pub description: String,
    /// Manifest version.
    #[serde(default = "one")]
    pub version: u32,
    /// Tools.
    pub tools: Vec<PackTool>,
}

fn one() -> u32 {
    1
}

/// One tool in a pack.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackTool {
    /// `<pack id>_...`.
    pub name: String,
    /// Short title.
    pub title: String,
    /// What it does, for the assistant.
    pub description: String,
    /// Risk.
    pub risk: Risk,
    /// Script language.
    pub language: Language,
    /// Seconds.
    #[serde(default)]
    pub timeout_s: Option<u64>,
    /// JSON Schema text.
    pub input_schema: String,
    /// Script source.
    pub script: String,
    /// Parsed schema (filled by [`Pack::parse`]).
    #[serde(skip)]
    pub schema: Map<String, Value>,
}

/// A manifest problem, naming the field so the portal can point at it.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct PackError {
    /// Field name for the portal (`manifest` when it's the whole file).
    pub field: &'static str,
    /// Sentence.
    pub message: String,
}

fn err(field: &'static str, message: impl Into<String>) -> PackError {
    PackError {
        field,
        message: message.into(),
    }
}

fn ident_ok(s: &str, max: usize) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && s.len() <= max
        && s.len() >= 2
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

impl Pack {
    /// Parse and validate a TOML manifest.
    pub fn parse(text: &str) -> Result<Self, PackError> {
        if text.len() > 512 * 1024 {
            return Err(err("manifest", "The manifest is over 512 KB."));
        }
        let mut pack: Pack = toml::from_str(text).map_err(|e| {
            err(
                "manifest",
                format!("The manifest isn't valid TOML: {}", e.message()),
            )
        })?;
        if !ident_ok(&pack.id, 24) {
            return Err(err(
                "id",
                "The id must be 2 to 24 lowercase letters, digits or underscores, starting with a letter.",
            ));
        }
        if crate::util::clean_name(&pack.name, 60).is_none() {
            return Err(err("name", "The name must be 1 to 60 characters."));
        }
        if pack.tools.is_empty() || pack.tools.len() > 40 {
            return Err(err("tools", "A pack needs 1 to 40 tools."));
        }
        let prefix = format!("{}_", pack.id);
        let mut seen = std::collections::HashSet::new();
        for tool in &mut pack.tools {
            if !tool.name.starts_with(&prefix) || !ident_ok(&tool.name, 48) {
                return Err(err(
                    "tools",
                    format!(
                        "Tool \u{201c}{}\u{201d} must start with \u{201c}{prefix}\u{201d} and use at most 48 lowercase letters, digits or underscores.",
                        tool.name
                    ),
                ));
            }
            if !seen.insert(tool.name.clone()) {
                return Err(err(
                    "tools",
                    format!("Two tools are named \u{201c}{}\u{201d}.", tool.name),
                ));
            }
            if tool.description.trim().is_empty() || tool.description.len() > 2000 {
                return Err(err(
                    "tools",
                    format!(
                        "\u{201c}{}\u{201d} needs a description under 2000 characters.",
                        tool.name
                    ),
                ));
            }
            if !(1..=300).contains(&tool.timeout_s.unwrap_or(30)) {
                return Err(err(
                    "tools",
                    format!("\u{201c}{}\u{201d} timeout_s must be 1 to 300.", tool.name),
                ));
            }
            let schema: Value = serde_json::from_str(&tool.input_schema).map_err(|e| {
                err(
                    "tools",
                    format!(
                        "The input_schema of \u{201c}{}\u{201d} isn't valid JSON: {e}",
                        tool.name
                    ),
                )
            })?;
            let Value::Object(schema) = schema else {
                return Err(err(
                    "tools",
                    format!(
                        "The input_schema of \u{201c}{}\u{201d} must be an object.",
                        tool.name
                    ),
                ));
            };
            if schema.get("type").and_then(Value::as_str) != Some("object") {
                return Err(err(
                    "tools",
                    format!(
                        "The input_schema of \u{201c}{}\u{201d} must have \"type\": \"object\".",
                        tool.name
                    ),
                ));
            }
            if tool.risk == Risk::Send {
                let requires = schema
                    .get("required")
                    .and_then(Value::as_array)
                    .is_some_and(|r| r.iter().any(|v| v == "request_id"));
                if !requires {
                    return Err(err(
                        "tools",
                        format!(
                            "\u{201c}{}\u{201d} sends things, so it must require a request_id.",
                            tool.name
                        ),
                    ));
                }
            }
            if tool.script.trim().is_empty() {
                return Err(err(
                    "tools",
                    format!("\u{201c}{}\u{201d} has no script.", tool.name),
                ));
            }
            tool.schema = schema;
        }
        Ok(pack)
    }

    /// Find a tool.
    pub fn tool(&self, name: &str) -> Option<&PackTool> {
        self.tools.iter().find(|t| t.name == name)
    }
}

/// The built-in packs, parsed once.
#[derive(Debug, Default)]
pub struct Catalog {
    /// By pack id.
    pub packs: HashMap<String, Pack>,
}

impl Catalog {
    /// Load every embedded pack. A broken built-in pack is logged and skipped.
    pub fn builtin() -> Self {
        let mut packs = HashMap::new();
        for name in Builtin::iter() {
            let Some(file) = Builtin::get(&name) else {
                continue;
            };
            match std::str::from_utf8(&file.data)
                .map_err(|_| err("manifest", "not UTF-8"))
                .and_then(Pack::parse)
            {
                Ok(pack) => {
                    packs.insert(pack.id.clone(), pack);
                }
                Err(e) => tracing::error!(pack = %name, error = %e, "built-in pack is invalid"),
            }
        }
        Self { packs }
    }

    /// Sorted by name.
    pub fn sorted(&self) -> Vec<&Pack> {
        let mut v: Vec<&Pack> = self.packs.values().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }
}

/// Check arguments against the subset of JSON Schema packs use. Returns a sentence.
pub fn validate_args(schema: &Map<String, Value>, args: &Map<String, Value>) -> Result<(), String> {
    check(
        &Value::Object(schema.clone()),
        &Value::Object(args.clone()),
        "arguments",
    )
}

fn check(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    let Some(schema) = schema.as_object() else {
        return Ok(());
    };
    if let Some(choices) = schema.get("enum").and_then(Value::as_array)
        && !choices.contains(value)
    {
        return Err(format!(
            "{path} must be one of {}.",
            Value::Array(choices.clone())
        ));
    }
    let ty = schema.get("type").and_then(Value::as_str);
    let ok = match ty {
        Some("object") => value.is_object(),
        Some("array") => value.is_array(),
        Some("string") => value.is_string(),
        Some("integer") => value.as_i64().is_some() || value.as_u64().is_some(),
        Some("number") => value.is_number(),
        Some("boolean") => value.is_boolean(),
        Some("null") => value.is_null(),
        _ => true,
    };
    if !ok {
        return Err(format!(
            "{path} must be a{} {}.",
            if ty == Some("integer") || ty == Some("object") || ty == Some("array") {
                "n"
            } else {
                ""
            },
            ty.unwrap_or("value")
        ));
    }
    if let Some(n) = value.as_f64() {
        if let Some(min) = schema.get("minimum").and_then(Value::as_f64)
            && n < min
        {
            return Err(format!("{path} must be at least {min}."));
        }
        if let Some(max) = schema.get("maximum").and_then(Value::as_f64)
            && n > max
        {
            return Err(format!("{path} must be at most {max}."));
        }
    }
    if let Some(s) = value.as_str() {
        if let Some(max) = schema.get("maxLength").and_then(Value::as_u64)
            && s.chars().count() as u64 > max
        {
            return Err(format!("{path} must be at most {max} characters."));
        }
        if let Some(min) = schema.get("minLength").and_then(Value::as_u64)
            && (s.chars().count() as u64) < min
        {
            return Err(format!("{path} must be at least {min} characters."));
        }
    }
    if let (Some(items), Some(arr)) = (schema.get("items"), value.as_array()) {
        if let Some(max) = schema.get("maxItems").and_then(Value::as_u64)
            && arr.len() as u64 > max
        {
            return Err(format!("{path} can have at most {max} items."));
        }
        for (i, item) in arr.iter().enumerate() {
            check(items, item, &format!("{path}[{i}]"))?;
        }
    }
    if let Some(obj) = value.as_object() {
        let props = schema.get("properties").and_then(Value::as_object);
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for key in required.iter().filter_map(Value::as_str) {
                if !obj.contains_key(key) {
                    return Err(format!("{key} is required."));
                }
            }
        }
        for (key, v) in obj {
            match props.and_then(|p| p.get(key)) {
                Some(sub) => check(sub, v, key)?,
                None if schema.get("additionalProperties") == Some(&Value::Bool(false)) => {
                    return Err(format!("{key} isn't an argument of this tool."));
                }
                None => {}
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const GOOD: &str = r#"
id = "notes"
name = "Notes"
bundle_id = "com.apple.Notes"
description = "Notes."
[[tools]]
name = "notes_send"
title = "Send"
description = "Sends."
risk = "send"
language = "javascript"
input_schema = '{"type":"object","properties":{"request_id":{"type":"string"},"n":{"type":"integer","minimum":1,"maximum":5},"mode":{"enum":["a","b"]},"tags":{"type":"array","items":{"type":"string","maxLength":3}}},"required":["request_id"],"additionalProperties":false}'
script = "function run(argv) { return argv[0]; }"
"#;

    #[test]
    fn manifests_are_validated_with_field_names() {
        let pack = Pack::parse(GOOD).unwrap();
        assert_eq!(pack.tools[0].schema["type"], "object");
        let bad_prefix = GOOD.replace("name = \"notes_send\"", "name = \"mail_send\"");
        assert_eq!(Pack::parse(&bad_prefix).unwrap_err().field, "tools");
        let no_request_id = GOOD.replace(r#""required":["request_id"],"#, "");
        assert!(
            Pack::parse(&no_request_id)
                .unwrap_err()
                .message
                .contains("request_id")
        );
        assert_eq!(Pack::parse("id = 'X'").unwrap_err().field, "manifest");
        assert_eq!(
            Pack::parse(&GOOD.replace("id = \"notes\"", "id = \"Notes\""))
                .unwrap_err()
                .field,
            "id"
        );
    }

    #[test]
    fn arguments_are_checked_against_the_schema() {
        let pack = Pack::parse(GOOD).unwrap();
        let s = &pack.tools[0].schema;
        let ok = |v: Value| validate_args(s, v.as_object().unwrap());
        assert!(ok(json!({"request_id":"x","n":3,"mode":"a","tags":["ab"]})).is_ok());
        assert_eq!(ok(json!({})).unwrap_err(), "request_id is required.");
        assert!(
            ok(json!({"request_id":"x","n":9}))
                .unwrap_err()
                .contains("at most 5")
        );
        assert!(
            ok(json!({"request_id":"x","n":"3"}))
                .unwrap_err()
                .contains("integer")
        );
        assert!(
            ok(json!({"request_id":"x","mode":"c"}))
                .unwrap_err()
                .contains("one of")
        );
        assert!(
            ok(json!({"request_id":"x","tags":["abcd"]}))
                .unwrap_err()
                .contains("tags[0]")
        );
        assert!(
            ok(json!({"request_id":"x","extra":1}))
                .unwrap_err()
                .contains("isn't an argument")
        );
    }

    #[test]
    fn every_builtin_pack_is_valid_and_tool_names_are_unique() {
        let catalog = Catalog::builtin();
        let files = Builtin::iter().count();
        assert_eq!(
            catalog.packs.len(),
            files,
            "a built-in pack failed validation"
        );
        let mut names = std::collections::HashSet::new();
        for pack in catalog.packs.values() {
            for t in &pack.tools {
                assert!(names.insert(t.name.clone()), "duplicate {}", t.name);
            }
        }
    }
}
