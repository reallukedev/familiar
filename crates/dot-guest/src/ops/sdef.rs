//! Condense an `sdef` scripting dictionary into short one-line signatures.
use dot_wire::{AppDictionary, Suite};
use roxmltree::{Document, Node, ParsingOptions};

/// Longest description kept per item.
const MAX_DESCRIPTION: usize = 160;
/// Most commands or classes kept per suite.
const MAX_ITEMS: usize = 300;
/// Rough cap on the whole condensed dictionary, in characters.
const MAX_TOTAL: usize = 400_000;
/// Where `xi:include`d standard suites may come from.
const INCLUDE_ROOT: &str = "/System/Library/ScriptingDefinitions/";

/// Condense `xml`. `include` loads an `xi:include` target (a local path under
/// `/System/Library/ScriptingDefinitions/`); it returns `None` to skip it.
pub fn condense(
    app: &str,
    xml: &str,
    include: &dyn Fn(&str) -> Option<String>,
) -> Result<AppDictionary, String> {
    let doc = parse(xml)?;
    let mut suites = Vec::new();
    collect_suites(doc.root_element(), include, &mut suites, 0);
    let mut total = 0;
    let mut kept = Vec::with_capacity(suites.len());
    for suite in suites {
        total += suite.name.len()
            + suite.commands.iter().map(String::len).sum::<usize>()
            + suite.classes.iter().map(String::len).sum::<usize>();
        if total > MAX_TOTAL {
            break;
        }
        kept.push(suite);
    }
    Ok(AppDictionary {
        app: app.to_owned(),
        suites: kept,
    })
}

fn parse(xml: &str) -> Result<Document<'_>, String> {
    Document::parse_with_options(
        xml,
        ParsingOptions {
            allow_dtd: true,
            ..ParsingOptions::default()
        },
    )
    .map_err(|e| format!("the scripting dictionary is not valid XML: {e}"))
}

fn collect_suites(
    root: Node<'_, '_>,
    include: &dyn Fn(&str) -> Option<String>,
    out: &mut Vec<Suite>,
    nesting: u8,
) {
    for child in root.children().filter(Node::is_element) {
        match child.tag_name().name() {
            "suite" if !hidden(child) => out.push(suite(child)),
            "include" if nesting == 0 => {
                let Some(href) = child.attribute("href") else {
                    continue;
                };
                let path = href.strip_prefix("file://").unwrap_or(href);
                if !path.starts_with(INCLUDE_ROOT) || path.contains("..") {
                    continue;
                }
                if let Some(text) = include(path)
                    && let Ok(doc) = parse(&text)
                {
                    collect_suites(doc.root_element(), include, out, nesting + 1);
                }
            }
            _ => {}
        }
    }
}

fn hidden(node: Node<'_, '_>) -> bool {
    node.attribute("hidden") == Some("yes")
}

fn describe(node: Node<'_, '_>) -> String {
    let text = node.attribute("description").unwrap_or("").trim();
    let mut text: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if crate::util::truncate_chars(&mut text, MAX_DESCRIPTION) {
        text.push_str("...");
    }
    text
}

/// `type="text"`, or `<type type="text" list="yes"/>` children joined with `|`.
fn type_of(node: Node<'_, '_>) -> String {
    if let Some(t) = node.attribute("type") {
        return t.to_owned();
    }
    let types: Vec<String> = node
        .children()
        .filter(|c| c.has_tag_name("type"))
        .filter_map(|c| {
            let t = c.attribute("type")?;
            Some(if c.attribute("list") == Some("yes") {
                format!("list of {t}")
            } else {
                t.to_owned()
            })
        })
        .collect();
    if types.is_empty() {
        "any".to_owned()
    } else {
        types.join(" | ")
    }
}

fn with_description(sig: String, node: Node<'_, '_>) -> String {
    let d = describe(node);
    if d.is_empty() {
        sig
    } else {
        format!("{sig}: {d}")
    }
}

fn suite(node: Node<'_, '_>) -> Suite {
    let mut commands = Vec::new();
    let mut classes = Vec::new();
    for item in node.children().filter(|c| c.is_element() && !hidden(*c)) {
        match item.tag_name().name() {
            "command" if commands.len() < MAX_ITEMS => commands.push(command(item)),
            "class" | "class-extension" | "record-type" if classes.len() < MAX_ITEMS => {
                classes.push(class(item));
            }
            "enumeration" if classes.len() < MAX_ITEMS => classes.push(enumeration(item)),
            _ => {}
        }
    }
    Suite {
        name: node.attribute("name").unwrap_or("Unnamed suite").to_owned(),
        commands,
        classes,
    }
}

fn command(node: Node<'_, '_>) -> String {
    let mut params = Vec::new();
    for p in node.children().filter(|c| c.is_element() && !hidden(*c)) {
        let optional = p.attribute("optional") == Some("yes");
        let param = match p.tag_name().name() {
            "direct-parameter" => format!("direct: {}", type_of(p)),
            "parameter" => format!("{}: {}", p.attribute("name").unwrap_or("?"), type_of(p)),
            _ => continue,
        };
        params.push(if optional {
            format!("[{param}]")
        } else {
            param
        });
    }
    let result = node
        .children()
        .find(|c| c.has_tag_name("result"))
        .map(|r| format!(" -> {}", type_of(r)))
        .unwrap_or_default();
    let name = node.attribute("name").unwrap_or("?");
    with_description(format!("{name}({}){result}", params.join(", ")), node)
}

fn class(node: Node<'_, '_>) -> String {
    let mut props = Vec::new();
    let mut elements = Vec::new();
    for c in node.children().filter(|c| c.is_element() && !hidden(*c)) {
        match c.tag_name().name() {
            "property" => {
                let ro = if c.attribute("access") == Some("r") {
                    " (r/o)"
                } else {
                    ""
                };
                props.push(format!(
                    "{}: {}{ro}",
                    c.attribute("name").unwrap_or("?"),
                    type_of(c)
                ));
            }
            "element" => elements.push(c.attribute("type").unwrap_or("?").to_owned()),
            _ => {}
        }
    }
    let head = match node.tag_name().name() {
        "class-extension" => format!("extends {}", node.attribute("extends").unwrap_or("?")),
        "record-type" => format!("record {}", node.attribute("name").unwrap_or("?")),
        _ => {
            let name = node.attribute("name").unwrap_or("?");
            match node.attribute("inherits") {
                Some(parent) => format!("{name} < {parent}"),
                None => name.to_owned(),
            }
        }
    };
    let mut body = props.join(", ");
    if !elements.is_empty() {
        if !body.is_empty() {
            body.push_str("; ");
        }
        body.push_str("elements: ");
        body.push_str(&elements.join(", "));
    }
    with_description(format!("{head} {{ {body} }}"), node)
}

fn enumeration(node: Node<'_, '_>) -> String {
    let values: Vec<&str> = node
        .children()
        .filter(|c| c.has_tag_name("enumerator") && !hidden(*c))
        .filter_map(|c| c.attribute("name"))
        .collect();
    format!(
        "enum {} = {}",
        node.attribute("name").unwrap_or("?"),
        values.join(" | ")
    )
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        reason = "Test assertions fail immediately on malformed setup"
    )]
    use super::*;

    const SDEF: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE dictionary SYSTEM "file://localhost/System/Library/DTDs/sdef.dtd">
<dictionary title="Notes Terminology" xmlns:xi="http://www.w3.org/2003/XInclude">
  <xi:include href="file:///System/Library/ScriptingDefinitions/CocoaStandard.sdef" xpointer="xpointer(/dictionary/suite)"/>
  <xi:include href="file:///Users/evil/steal.sdef"/>
  <suite name="Notes Suite" code="note" description="Terms   for
      notes.">
    <command name="show" code="notesshw" description="Show an object in the UI">
      <direct-parameter type="specifier" description="the object"/>
      <parameter name="separately" code="sepa" type="boolean" optional="yes"/>
      <result><type type="note" list="yes"/></result>
    </command>
    <command name="secret" code="notessec" hidden="yes"/>
    <class name="note" code="note" inherits="item" plural="notes" description="a note">
      <property name="name" code="pnam" type="text"/>
      <property name="id" code="ID  " type="text" access="r"/>
      <property name="internal" code="intr" type="text" hidden="yes"/>
      <element type="attachment"/>
    </class>
    <class-extension extends="application">
      <element type="note"/>
    </class-extension>
    <enumeration name="saveable file format" code="savf">
      <enumerator name="native format" code="item"/>
      <enumerator name="text" code="ctxt"/>
    </enumeration>
  </suite>
</dictionary>"#;

    const STANDARD: &str = r#"<?xml version="1.0"?><dictionary>
      <suite name="Standard Suite" code="????">
        <command name="close" code="coreclos" description="Close a document.">
          <direct-parameter type="specifier"/>
          <parameter name="saving" code="savo" type="save options" optional="yes"/>
        </command>
      </suite></dictionary>"#;

    fn condensed() -> AppDictionary {
        condense("Notes", SDEF, &|path| {
            assert_eq!(
                path,
                "/System/Library/ScriptingDefinitions/CocoaStandard.sdef"
            );
            Some(STANDARD.to_owned())
        })
        .unwrap()
    }

    #[test]
    fn standard_suite_include_is_inlined_and_foreign_includes_ignored() {
        let dict = condensed();
        let names: Vec<_> = dict.suites.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Standard Suite", "Notes Suite"]);
        assert_eq!(
            dict.suites[0].commands,
            ["close(direct: specifier, [saving: save options]): Close a document."]
        );
    }

    #[test]
    fn commands_classes_and_enums_condense_to_one_line_each() {
        let dict = condensed();
        let notes = &dict.suites[1];
        assert_eq!(
            notes.commands,
            [
                "show(direct: specifier, [separately: boolean]) -> list of note: Show an object in the UI"
            ]
        );
        assert_eq!(
            notes.classes,
            [
                "note < item { name: text, id: text (r/o); elements: attachment }: a note",
                "extends application { elements: note }",
                "enum saveable file format = native format | text",
            ]
        );
    }

    #[test]
    fn malformed_xml_is_an_error() {
        assert!(condense("X", "<dictionary><suite>", &|_| None).is_err());
    }
}
