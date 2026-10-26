//! Approval modes: one choice that sets every permission group of a Mac at once.
//!
//! - `ask`: every group asks first.
//! - `changes` (new Macs): groups that only read are allowed; anything that changes or
//!   sends asks first.
//! - `all`: everything is allowed.
//!
//! A group the owner set to Never stays Never in every mode. When the groups no longer
//! match any mode (the owner changed one by hand), the mode reads as `custom`.
//!
//! App servers (`mcp:` groups) only follow a mode downwards: Ask for Everything (and Ask
//! Before Changes, for servers that change things) makes an allowed server ask, but no
//! mode turns a server on or makes it allowed. They don't count toward detecting the mode.
use crate::{
    packs::{Pack, Risk},
    state::App,
    tools::{self, catalog},
};

/// The modes an owner can choose.
pub const MODES: &[&str] = &["ask", "changes", "all"];

/// The mode new Macs start in.
pub const DEFAULT: &str = "changes";

/// Whether every built-in tool in a group only reads.
pub fn builtin_read_only(group: &str) -> bool {
    let mut tools = catalog::builtins()
        .iter()
        .filter(|t| t.group == group)
        .peekable();
    tools.peek().is_some() && tools.all(|t| t.risk == Risk::Read)
}

/// Whether every tool of a pack only reads.
pub fn pack_read_only(pack: &Pack) -> bool {
    !pack.tools.is_empty() && pack.tools.iter().all(|t| t.risk == Risk::Read)
}

/// The setting a group gets under `mode`, given its current setting. Never stays Never.
pub fn group_mode(mode: &str, read_only: bool, current: &str) -> &'static str {
    if current == "deny" {
        return "deny";
    }
    match mode {
        "ask" => "ask",
        "all" => "allow",
        _ if read_only => "allow",
        _ => "ask",
    }
}

/// One group as the modes see it.
#[derive(Debug, Clone)]
pub struct GroupState {
    /// Group id (`observe`, `pack:mail`, ...).
    pub id: String,
    /// Only reads.
    pub read_only: bool,
    /// Effective setting: `allow`, `ask` or `deny`.
    pub mode: String,
}

/// The mode the groups match, or `custom`. `preferred` (the stored mode) wins a tie,
/// which happens when every group is Never.
pub fn detect(preferred: Option<&str>, groups: &[GroupState]) -> &'static str {
    let matches = |m: &str| {
        groups
            .iter()
            .all(|g| g.mode == group_mode(m, g.read_only, &g.mode))
    };
    let preferred = preferred.and_then(|p| MODES.iter().find(|m| **m == p));
    preferred
        .into_iter()
        .chain(MODES.iter())
        .find(|m| matches(m))
        .copied()
        .unwrap_or("custom")
}

/// Every group of a Mac (built-in, then enabled packs) with its effective setting.
pub async fn groups(app: &App, instance_id: &str) -> anyhow::Result<Vec<GroupState>> {
    let modes = tools::modes(app, instance_id).await?;
    let mut out: Vec<GroupState> = catalog::GROUPS
        .iter()
        .map(|g| GroupState {
            id: g.id.to_owned(),
            read_only: builtin_read_only(g.id),
            mode: tools::mode_of(&modes, g.id),
        })
        .collect();
    for p in tools::enabled_packs(app, instance_id).await? {
        let id = format!("pack:{}", p.id);
        out.push(GroupState {
            mode: tools::mode_of(&modes, &id),
            read_only: pack_read_only(&p),
            id,
        });
    }
    Ok(out)
}

/// The Mac's current mode: `ask`, `changes`, `all` or `custom`.
pub async fn current(app: &App, instance_id: &str) -> anyhow::Result<&'static str> {
    let groups = groups(app, instance_id).await?;
    let stored = stored(app, instance_id).await?;
    Ok(detect(stored.as_deref(), &groups))
}

async fn stored(app: &App, instance_id: &str) -> anyhow::Result<Option<String>> {
    let iid = instance_id.to_owned();
    Ok(app
        .db
        .call(move |c| {
            c.query_row(
                "SELECT approval_mode FROM instances WHERE id = ?1",
                [iid],
                |r| r.get(0),
            )
        })
        .await?)
}

/// The setting an app server gets under `mode`: only ever lowered from Allow to Ask.
pub fn server_mode(mode: &str, read_only: bool, current: &str) -> Option<&'static str> {
    let lower = current == "allow" && (mode == "ask" || mode == "changes" && !read_only);
    lower.then_some("ask")
}

/// Set every group for `mode` in one transaction and remember the mode, so groups that
/// appear later (a new app) follow it too.
pub async fn set(app: &App, instance_id: &str, mode: &str) -> anyhow::Result<()> {
    anyhow::ensure!(MODES.contains(&mode), "unknown approval mode {mode}");
    let mut rows: Vec<(String, &'static str)> = groups(app, instance_id)
        .await?
        .into_iter()
        .map(|g| {
            let m = group_mode(mode, g.read_only, &g.mode);
            (g.id, m)
        })
        .collect();
    let modes = tools::modes(app, instance_id).await?;
    for entry in tools::appmcp::load(app, instance_id).await?.servers {
        let id = tools::appmcp::group_id(&entry.server.id);
        let read_only = !entry.tools.is_empty() && entry.tools.iter().all(|t| t.read_only);
        if let Some(m) = server_mode(mode, read_only, &tools::mode_of(&modes, &id)) {
            rows.push((id, m));
        }
    }
    let (iid, mode) = (instance_id.to_owned(), mode.to_owned());
    app.db
        .call(move |c| {
            let tx = c.transaction()?;
            for (g, m) in rows {
                tx.execute(
                    "INSERT INTO permissions (instance_id, grp, mode) VALUES (?1, ?2, ?3)
                     ON CONFLICT(instance_id, grp) DO UPDATE SET mode = excluded.mode",
                    rusqlite::params![iid, g, m],
                )?;
            }
            tx.execute(
                "UPDATE instances SET approval_mode = ?2 WHERE id = ?1",
                rusqlite::params![iid, mode],
            )?;
            tx.commit()
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(id: &str, read_only: bool, mode: &str) -> GroupState {
        GroupState {
            id: id.into(),
            read_only,
            mode: mode.into(),
        }
    }

    #[test]
    fn read_only_groups_come_from_the_catalog() {
        assert!(builtin_read_only("observe"));
        assert!(builtin_read_only("files_read"));
        assert!(!builtin_read_only("files_write"));
        assert!(!builtin_read_only("control"));
        // Reads and replaces the clipboard: one group, so it counts as a change.
        assert!(!builtin_read_only("clipboard"));
        assert!(!builtin_read_only("no_such_group"));
    }

    #[test]
    fn each_mode_maps_groups_and_never_stays_never() {
        assert_eq!(group_mode("ask", true, "allow"), "ask");
        assert_eq!(group_mode("ask", false, "allow"), "ask");
        assert_eq!(group_mode("changes", true, "ask"), "allow");
        assert_eq!(group_mode("changes", false, "allow"), "ask");
        assert_eq!(group_mode("all", false, "ask"), "allow");
        for m in MODES {
            assert_eq!(group_mode(m, false, "deny"), "deny");
            assert_eq!(group_mode(m, true, "deny"), "deny");
        }
    }

    #[test]
    fn modes_only_ever_make_an_app_server_ask() {
        assert_eq!(server_mode("ask", true, "allow"), Some("ask"));
        assert_eq!(server_mode("changes", false, "allow"), Some("ask"));
        assert_eq!(server_mode("changes", true, "allow"), None);
        for m in MODES {
            assert_eq!(server_mode(m, false, "deny"), None, "never turned on");
            assert_eq!(server_mode(m, false, "ask"), None, "never allowed");
        }
    }

    #[test]
    fn detects_the_mode_or_custom() {
        let changes = [
            g("observe", true, "allow"),
            g("control", false, "ask"),
            g("outgoing", false, "deny"),
        ];
        assert_eq!(detect(None, &changes), "changes");
        let all = [g("observe", true, "allow"), g("control", false, "allow")];
        assert_eq!(detect(None, &all), "all");
        let ask = [g("observe", true, "ask"), g("control", false, "ask")];
        assert_eq!(detect(Some("all"), &ask), "ask");
        let custom = [g("observe", true, "ask"), g("control", false, "allow")];
        assert_eq!(detect(Some("changes"), &custom), "custom");
        // Everything Never matches every mode; the stored one wins.
        let never = [g("observe", true, "deny"), g("control", false, "deny")];
        assert_eq!(detect(Some("all"), &never), "all");
        assert_eq!(detect(None, &never), "ask");
    }
}
