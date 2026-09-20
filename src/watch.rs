//! Following herdr's focused tab into the matching AeroSpace workspace.
//!
//! herdr has no tab-focus event to subscribe to — protocol 22 offers only
//! `pane.output_matched`, `pane.agent_status_changed` and
//! `pane.scroll_changed` — so the focused tab is polled. `herdr tab list`
//! costs about 7ms, which is cheap enough to ask several times a second.
//!
//! The decision is separated from the polling so that every reason to do
//! nothing is testable. Refusing is the common case and the important one: a
//! wrong switch empties the screen.

use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    event: String,
}

#[derive(Deserialize)]
struct AeroEvent {
    #[serde(default, rename = "_event")]
    event: String,
    #[serde(default)]
    workspace: String,
}

/// The workspace AeroSpace has just moved to, from one of its own event lines.
///
/// `aerospace subscribe` prints JSON lines with the event name under `_event`.
/// Only a workspace change is of interest: it is the moment the follow list
/// needs to catch up, whoever caused it — a keybinding, a click, or sling.
pub fn workspace_changed(line: &str) -> Option<String> {
    let e: AeroEvent = serde_json::from_str(line).ok()?;
    if e.event == "focused-workspace-changed" && !e.workspace.is_empty() {
        Some(e.workspace)
    } else {
        None
    }
}

/// Is this pushed line a tab having been focused?
///
/// The subscription is named `tab.focused`; the event arrives as
/// `tab_focused`. The acknowledgement line carries no `event` field at all.
pub fn is_tab_focused(line: &str) -> bool {
    serde_json::from_str::<Envelope>(line)
        .map(|e| e.event == "tab_focused")
        .unwrap_or(false)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Why nothing happened. Worth carrying: in a watcher that mostly
    /// declines, "it did nothing" and "it did nothing for a good reason" need
    /// to be distinguishable.
    Hold(&'static str),
    Switch(String),
}

/// Decide whether the focused herdr tab should pull AeroSpace across.
///
/// `settled` is the tab seen on the previous poll. Requiring two consecutive
/// sightings debounces flicking through tabs, which would otherwise queue a
/// workspace switch per tab passed through.
pub fn decide(
    focused_tab: Option<&str>,
    settled: Option<&str>,
    current_workspace: &str,
    counts: &BTreeMap<String, usize>,
) -> Action {
    let Some(tab) = focused_tab else {
        return Action::Hold("no focused herdr tab");
    };
    if settled != Some(tab) {
        return Action::Hold("still moving");
    }
    if tab == current_workspace {
        return Action::Hold("already there");
    }
    // The one that matters. AeroSpace shows a workspace by restoring its
    // windows; a workspace with none is a blank screen, and getting back costs
    // a full restore of wherever you came from.
    match counts.get(tab) {
        Some(n) if *n > 0 => Action::Switch(tab.to_string()),
        _ => Action::Hold("that task holds no windows yet"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(list: &[(&str, usize)]) -> BTreeMap<String, usize> {
        list.iter().map(|(w, n)| (w.to_string(), *n)).collect()
    }

    #[test]
    fn reads_an_aerospace_workspace_change() {
        let line = r#"{"_event":"focused-workspace-changed","prevWorkspace":"M","workspace":"t-mail"}"#;
        assert_eq!(workspace_changed(line).as_deref(), Some("t-mail"));
    }

    #[test]
    fn ignores_other_aerospace_events() {
        assert!(workspace_changed(r#"{"_event":"focus-changed","windowId":28218,"workspace":"M"}"#).is_none());
        assert!(workspace_changed(r#"{"_event":"mode-changed","mode":"main"}"#).is_none());
        assert!(workspace_changed("not json").is_none());
    }

    #[test]
    fn recognises_a_pushed_tab_focus() {
        let event = r#"{"data":{"tab_id":"w7:tA","type":"tab_focused","workspace_id":"w7"},"event":"tab_focused"}"#;
        assert!(is_tab_focused(event));
    }

    #[test]
    fn ignores_the_acknowledgement_and_other_events() {
        assert!(!is_tab_focused(r#"{"id":"sling-watch","result":{"type":"subscription_started"}}"#));
        assert!(!is_tab_focused(r#"{"event":"pane_focused","data":{}}"#));
        assert!(!is_tab_focused("not json"));
        assert!(!is_tab_focused(""));
    }

    #[test]
    fn follows_a_settled_tab_to_a_workspace_with_windows() {
        let action = decide(
            Some("t-mail"),
            Some("t-mail"),
            "infra",
            &counts(&[("infra", 40), ("t-mail", 2)]),
        );
        assert_eq!(action, Action::Switch("t-mail".into()));
    }

    #[test]
    fn refuses_to_show_an_empty_workspace() {
        // Switching here would hide everything on screen and cost a full
        // restore to undo.
        let action = decide(Some("t-forms"), Some("t-forms"), "infra", &counts(&[("infra", 40)]));
        assert_eq!(action, Action::Hold("that task holds no windows yet"));
    }

    #[test]
    fn waits_for_a_tab_to_settle() {
        // Flicking through tabs must not queue a switch for each one passed.
        let seen = counts(&[("t-mail", 2), ("t-pair", 1)]);
        assert_eq!(decide(Some("t-pair"), Some("t-mail"), "infra", &seen), Action::Hold("still moving"));
        assert_eq!(decide(Some("t-pair"), Some("t-pair"), "infra", &seen), Action::Switch("t-pair".into()));
    }

    #[test]
    fn does_nothing_when_already_on_that_workspace() {
        let action = decide(Some("t-mail"), Some("t-mail"), "t-mail", &counts(&[("t-mail", 2)]));
        assert_eq!(action, Action::Hold("already there"));
    }

    #[test]
    fn does_nothing_without_a_focused_tab() {
        assert_eq!(decide(None, None, "infra", &counts(&[])), Action::Hold("no focused herdr tab"));
    }
}
