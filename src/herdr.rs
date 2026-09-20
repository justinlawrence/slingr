//! Herdr tabs as tasks.
//!
//! The model says a task is usually a herdr tab, so the tabs are read straight
//! from herdr rather than kept in step by hand. Renaming or adding a tab makes
//! it slingable immediately, with no config to edit.
//!
//! Labels use "/" where AeroSpace workspaces cannot, so `t/forms` arrives here
//! and becomes `t-forms` — the convention already in use.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde::Deserialize;

use crate::config::home;
use crate::picker::sanitise_workspace;

/// Absolute where we can find it. Under launchd the PATH is a bare minimum
/// and Homebrew is not on it, so a bare name would silently never resolve.
pub fn bin() -> String {
    for candidate in ["/opt/homebrew/bin/herdr", "/usr/local/bin/herdr"] {
        if std::path::Path::new(candidate).exists() {
            return candidate.to_string();
        }
    }
    "herdr".to_string()
}
pub const TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Deserialize)]
struct Reply {
    result: TabList,
}

#[derive(Deserialize)]
struct TabList {
    #[serde(default)]
    tabs: Vec<Tab>,
}

#[derive(Deserialize)]
struct Tab {
    #[serde(default)]
    label: String,
    #[serde(default)]
    focused: bool,
    #[serde(default)]
    tab_id: String,
}

/// A tab, as sling cares about it: the task it names and whether it is showing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabRef {
    pub task: String,
    pub id: String,
    pub focused: bool,
}

/// Task names from a `herdr tab list` reply.
///
/// A purely numeric label is herdr's default for a tab nobody has named, so it
/// describes no task and is left out.
pub fn parse_tabs(json: &str) -> Vec<String> {
    let Ok(reply) = serde_json::from_str::<Reply>(json) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for tab in reply.result.tabs {
        if tab.label.trim().is_empty() || tab.label.trim().chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let name = sanitise_workspace(&tab.label);
        if !name.is_empty() && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// Where the session socket lives, following herdr's own resolution order.
pub fn socket_path() -> PathBuf {
    if let Ok(explicit) = std::env::var("HERDR_SOCKET_PATH") {
        return PathBuf::from(explicit);
    }
    if let Ok(name) = std::env::var("HERDR_SESSION") {
        return home().join(format!(".config/herdr/sessions/{name}/herdr.sock"));
    }
    home().join(".config/herdr/herdr.sock")
}

/// Subscribe to events over the session socket.
///
/// Newline-delimited JSON, no handshake. The first line acknowledges the
/// subscription; every line after it is a pushed event. Events occurring
/// before the subscription is accepted are not replayed.
///
/// Note the spelling changes across the boundary: you subscribe to
/// `tab.focused` and receive `"event":"tab_focused"`.
pub fn subscribe(types: &[&str]) -> std::io::Result<UnixStream> {
    let mut stream = UnixStream::connect(socket_path())?;
    let subscriptions: Vec<serde_json::Value> =
        types.iter().map(|t| serde_json::json!({ "type": t })).collect();
    let request = serde_json::json!({
        "id": "sling-watch",
        "method": "events.subscribe",
        "params": { "subscriptions": subscriptions },
    });
    stream.write_all(format!("{request}\n").as_bytes())?;
    stream.flush()?;
    Ok(stream)
}

/// The task name of the focused tab, if any.
pub fn parse_focused(json: &str) -> Option<String> {
    let reply = serde_json::from_str::<Reply>(json).ok()?;
    let tab = reply.result.tabs.into_iter().find(|t| t.focused)?;
    let name = sanitise_workspace(&tab.label);
    if name.is_empty() || name.chars().all(|c| c.is_ascii_digit()) {
        None
    } else {
        Some(name)
    }
}

fn ask_tab_list() -> Option<String> {
    let child = Command::new(bin())
        .args(["tab", "list"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(TIMEOUT) {
        Ok(Ok(out)) if out.status.success() => {
            Some(String::from_utf8_lossy(&out.stdout).into_owned())
        }
        _ => None,
    }
}

/// The focused tab, asked of herdr.
pub fn focused() -> Option<String> {
    parse_focused(&ask_tab_list()?)
}

/// The tab naming a given task, if there is one.
///
/// Not every workspace has a tab — `house`, `mail` and the infra screen are
/// tasks with no agent behind them — so a miss is ordinary and means do
/// nothing.
pub fn tab_for(json: &str, task: &str) -> Option<TabRef> {
    let reply = serde_json::from_str::<Reply>(json).ok()?;
    reply
        .result
        .tabs
        .into_iter()
        .filter(|t| !t.tab_id.is_empty())
        .map(|t| TabRef {
            task: sanitise_workspace(&t.label),
            id: t.tab_id,
            focused: t.focused,
        })
        .find(|t| t.task == task)
}

/// Bring a tab to the front. False if herdr would not, or could not.
pub fn focus_tab(id: &str) -> bool {
    Command::new(bin())
        .args(["tab", "focus", id])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// The tab naming a task, asked of herdr.
pub fn tab_named(task: &str) -> Option<TabRef> {
    tab_for(&ask_tab_list()?, task)
}

/// Ask herdr for its tabs. Empty if herdr is not installed or not answering —
/// sling works without it, just with a shorter list.
pub fn tasks() -> Vec<String> {
    ask_tab_list().map(|json| parse_tabs(&json)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"id":"cli:tab:list","result":{"tabs":[
        {"label":"ac/app","tab_id":"w3:t1","focused":false},
        {"label":"t/gant-workload","tab_id":"w7:t4","focused":true},
        {"label":"1","tab_id":"w5:t1","focused":false},
        {"label":"arb/lsp","tab_id":"w7:t8","focused":false},
        {"label":"","tab_id":"w9:t1","focused":false},
        {"label":"ac/app","tab_id":"w3:t9","focused":false}
    ],"type":"tab_list"}}"#;

    #[test]
    fn turns_tab_labels_into_workspace_names() {
        assert_eq!(
            parse_tabs(SAMPLE),
            vec!["ac-app", "t-gant-workload", "arb-lsp"]
        );
    }

    #[test]
    fn finds_the_focused_tab() {
        assert_eq!(parse_focused(SAMPLE).as_deref(), Some("t-gant-workload"));
    }

    #[test]
    fn no_focused_tab_is_not_an_error() {
        let none = SAMPLE.replace("\"focused\":true", "\"focused\":false");
        assert!(parse_focused(&none).is_none());
        assert!(parse_focused("not json").is_none());
    }

    #[test]
    fn an_unnamed_focused_tab_names_no_task() {
        let numbered = r#"{"result":{"tabs":[{"label":"1","focused":true}]}}"#;
        assert!(parse_focused(numbered).is_none());
    }

    #[test]
    fn finds_the_tab_that_names_a_task() {
        let found = tab_for(SAMPLE, "t-gant-workload").expect("there is one");
        assert_eq!(found.id, "w7:t4");
        assert!(found.focused);

        let other = tab_for(SAMPLE, "arb-lsp").expect("there is one");
        assert_eq!(other.id, "w7:t8");
        assert!(!other.focused);
    }

    #[test]
    fn a_task_with_no_tab_is_ordinary() {
        // house, mail and the infra screen are tasks with nobody behind them.
        assert!(tab_for(SAMPLE, "house").is_none());
        assert!(tab_for("not json", "t-mail").is_none());
    }

    #[test]
    fn ignores_a_reply_it_cannot_read() {
        assert!(parse_tabs("not json").is_empty());
        assert!(parse_tabs("{}").is_empty());
        assert!(parse_tabs(r#"{"result":{}}"#).is_empty());
    }
}
