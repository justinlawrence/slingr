//! The native panel, as a `Prompt`.
//!
//! Presentation lives in `panel/main.swift`; this only hands it the rows and
//! reads back what was chosen. A request goes in on stdin as JSON, the chosen
//! ids come out on stdout, one per line, and a non-zero exit means cancelled.
//!
//! Why a panel rather than a dialog: AeroSpace does not manage panels, so it
//! belongs to no workspace and is never hidden or moved. It is also
//! non-activating, so AeroSpace still reports the window being slung as
//! focused while the panel is up — which the System Events dialog could not do.

use std::io::Write;
use std::process::{Command, Stdio};

use serde::Serialize;

use crate::dialog::Prompt;
use crate::picker::Row;
use crate::theme::{self, Theme};

/// Beside the sling binary, so the pair moves together.
///
/// `current_exe` hands back the path sling was *invoked* by, which is normally
/// `~/.local/bin/sling` — a symlink into the build directory. The panel lives
/// next to the real binary, not next to the link, so the link has to be
/// followed. Both are tried, resolved first, because a copied binary has no
/// link to follow.
pub fn binary() -> std::path::PathBuf {
    let invoked = std::env::current_exe().ok();
    let resolved = invoked.as_ref().and_then(|p| std::fs::canonicalize(p).ok());

    let candidates = [resolved.as_deref(), invoked.as_deref()]
        .into_iter()
        .flatten()
        .filter_map(|p| p.parent())
        .map(|dir| dir.join("slingr-panel"));

    let mut first = None;
    for candidate in candidates {
        if candidate.exists() {
            return candidate;
        }
        first.get_or_insert(candidate);
    }
    first.unwrap_or_else(|| "slingr-panel".into())
}

pub fn available() -> bool {
    binary().exists()
}

#[derive(Serialize)]
struct Request<'a> {
    title: &'a str,
    subtitle: &'a str,
    multi: bool,
    placeholder: &'a str,
    theme: Theme,
    #[serde(rename = "pinnedFolders")]
    pinned_folders: &'a [String],
    /// Which surface to draw — the panel's ordinary list, or the board.
    layout: &'a str,
    items: &'a [Row],
}

#[derive(Default)]
pub struct Panel;

impl Panel {
    fn ask_with(
        &self,
        rows: &[Row],
        title: &str,
        subtitle: &str,
        multi: bool,
        pinned_folders: &[String],
        layout: &str,
    ) -> Option<Vec<String>> {
        let request = Request {
            title,
            subtitle,
            multi,
            placeholder: if multi { "type to filter windows" } else { "type to filter tasks" },
            // Inherited from the terminal, so the panel matches the windows it
            // appears over rather than imposing a palette of its own.
            theme: theme::current(),
            pinned_folders,
            layout,
            items: rows,
        };
        let body = serde_json::to_vec(&request).ok()?;

        let mut child = Command::new(binary())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        child.stdin.take()?.write_all(&body).ok()?;

        let out = child.wait_with_output().ok()?;
        if !out.status.success() {
            return None; // cancelled
        }
        let chosen: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        (!chosen.is_empty()).then_some(chosen)
    }
}

impl Prompt for Panel {
    fn choose_rows(&self, rows: &[Row], title: &str, subtitle: &str) -> Option<String> {
        self.ask_with(rows, title, subtitle, false, &pinned_from(rows), "list")?.into_iter().next()
    }

    fn choose_many_rows(&self, rows: &[Row], title: &str, subtitle: &str) -> Option<Vec<String>> {
        self.ask_with(rows, title, subtitle, true, &pinned_from(rows), "list")
    }

    /// The board answers with as many lines as there were drags, so unlike
    /// every other single-choice question it keeps the whole list.
    fn choose_board(&self, rows: &[Row], title: &str, subtitle: &str) -> Option<Vec<String>> {
        self.ask_with(rows, title, subtitle, false, &pinned_from(rows), "board")
    }

    // The flat-string methods exist for the AppleScript dialog. The panel is
    // given rows directly and never needs them.
    fn choose(&self, _: &[String], _: &str, _: &str) -> Option<String> {
        None
    }
    fn choose_many(&self, _: &[String], _: &str, _: &str) -> Option<Vec<String>> {
        None
    }
    fn ask_text(&self, title: &str, prompt: &str) -> Option<String> {
        // One free-text field is not worth a second panel yet.
        crate::dialog::SystemEvents.ask_text(title, prompt)
    }
}


/// Which folders are pinned, read back off the rows so the panel need not be
/// told twice. A pinned folder is simply one that sorted ahead of the rest.
fn pinned_from(rows: &[Row]) -> Vec<String> {
    rows.iter().filter(|r| r.pinned).map(|r| r.id.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::picker::{Occupancy, Row};
    use std::collections::BTreeMap;

    /// The panel decodes this JSON in Swift, where a renamed field is not a
    /// compile error but a silently missing value — an icon stack that never
    /// draws, or a board that renders as a list. Pin the spelling here.
    #[test]
    fn the_request_is_spelled_the_way_the_panel_reads_it() {
        let held = BTreeMap::from([(
            "me-tax".to_string(),
            Occupancy { count: 2, stack: vec!["dev.zed.Zed".into()] },
        )]);
        let menu = crate::picker::build_menu(Some(&held), &[], &[], "", &[], &BTreeMap::new(), &[]);
        let rows: Vec<Row> = crate::app::tabs(crate::app::Mode::Board)
            .into_iter()
            .chain(menu.rows)
            .chain(std::iter::once(Row::space("t-empty")))
            .collect();

        let request = Request {
            title: "Every window",
            subtitle: "t-pair",
            multi: false,
            placeholder: "",
            theme: crate::theme::Theme::default(),
            pinned_folders: &[],
            layout: "board",
            items: &rows,
        };
        let json = serde_json::to_value(&request).expect("serialises");

        assert_eq!(json["layout"], "board");
        assert_eq!(json["subtitle"], "t-pair", "the board reads `here` off the subtitle");
        assert_eq!(json["pinnedFolders"], serde_json::json!([]), "camelCase, as Swift spells it");

        let items = json["items"].as_array().expect("items is a list");
        let task = items
            .iter()
            .find(|i| i["id"] == "me-tax")
            .expect("the task is listed");
        assert_eq!(task["stack"], serde_json::json!(["dev.zed.Zed"]));
        assert_eq!(task["count"], 2);

        let empty = items.iter().find(|i| i["id"] == "t-empty").expect("the empty task is listed");
        assert_eq!(empty["marker"], "space", "the board draws a tile for it");
        assert_eq!(empty["section"], "t-empty");

        let board_tab = items.iter().find(|i| i["id"] == "__board__").expect("the board tab");
        assert_eq!(board_tab["marker"], "tab");
        assert_eq!(board_tab["active"], true);

        // An absent stack must be absent, not null: Swift decodes `[String]?`
        // and an empty array would draw an empty run of icons.
        assert!(items.iter().all(|i| i.get("stack") != Some(&serde_json::json!([]))));
    }
}
