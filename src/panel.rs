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
        self.ask_with(rows, title, subtitle, false, &pinned_from(rows))?.into_iter().next()
    }

    fn choose_many_rows(&self, rows: &[Row], title: &str, subtitle: &str) -> Option<Vec<String>> {
        self.ask_with(rows, title, subtitle, true, &pinned_from(rows))
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
