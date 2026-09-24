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

/// What a workspace change asks of the follow hook.
///
/// Every value here is a bug that happened. The hook used to do the same work
/// whatever caused the change, and each of these is a case where that was
/// wrong — see `docs/FINDINGS.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Catch {
    /// Another run of the hook is already in flight. Two at once move the same
    /// windows twice, in a race, each undoing part of what the other did.
    Busy,
    /// A switch of ours is still settling and this is not where it lands.
    /// AeroSpace reports the workspace it is on when the hook fires, which
    /// during a switch can still be the one being left.
    MidSwitch,
    /// Our own switch arriving. `goto` brought the followers before it moved,
    /// and herdr is where the change came from, so there is nothing to do but
    /// note it.
    Arrived,
    /// Somebody moved by some other means. Bring the followers across, and
    /// bring herdr's tab with them.
    Chase,
}

/// The rule, with the lock file and the clock taken out.
pub fn catch(another_is_running: bool, settling: crate::store::Settling) -> Catch {
    if another_is_running {
        return Catch::Busy;
    }
    match settling {
        crate::store::Settling::InFlight => Catch::MidSwitch,
        crate::store::Settling::Ours => Catch::Arrived,
        crate::store::Settling::Settled => Catch::Chase,
    }
}

impl Catch {
    /// Only one of the four does anything to your windows.
    pub fn brings_followers(self) -> bool {
        self == Catch::Chase
    }

    /// And only that one may push herdr around. Doing it in any other state is
    /// how slingr came to drag people backwards: it read the workspace being
    /// left, decided a person must have gone there, and focused the tab it had
    /// just come from — which fired another switch.
    pub fn moves_the_tab(self) -> bool {
        self == Catch::Chase
    }
}

/// Whether a switch that has just landed should look again before letting go
/// of the lock.
///
/// A tab clicked while the first switch was in flight is turned away by that
/// lock, so without a second look the earlier — and by then staler — intent
/// wins: you tap two tabs quickly and land on the one you did not choose last.
pub fn look_again(went_to: &str, tab_now: Option<&str>) -> bool {
    matches!(tab_now, Some(now) if now != went_to)
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
    _counts: &BTreeMap<String, usize>,
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
    // An empty task is fine to go to. This used to be refused, because on
    // AeroSpace 0.12 showing a workspace cost about three seconds per window
    // and the way back from a crowded one took minutes, so landing on a blank
    // screen was expensive to undo. Both costs are now a tenth of a second,
    // and the windows that belong everywhere arrive with you — so a new task
    // is not a blank screen, it is a terminal waiting to be worked in.
    Action::Switch(tab.to_string())
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
    fn a_task_with_nothing_in_it_yet_is_still_somewhere_to_go() {
        // Creating a tab and going there to start work is the ordinary case.
        // Refusing it was a defence against a cost that no longer exists.
        let action = decide(Some("t-forms"), Some("t-forms"), "infra", &counts(&[("infra", 40)]));
        assert_eq!(action, Action::Switch("t-forms".into()));
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

#[cfg(test)]
mod chain_tests {
    use super::{catch, look_again, Catch};
    use crate::store::Settling;

    #[test]
    fn an_ordinary_tab_switch_asks_the_hook_for_nothing() {
        // goto brought the followers before it moved and herdr is where the
        // change came from. Measured: this path used to re-list every window
        // to discover it had nothing to do.
        let c = catch(false, Settling::Ours);
        assert_eq!(c, Catch::Arrived);
        assert!(!c.brings_followers());
        assert!(!c.moves_the_tab());
    }

    #[test]
    fn a_workspace_reported_mid_switch_is_left_entirely_alone() {
        // Observed: goto went t-android -> ac-app and the hook fired saying
        // "t-android". Treating that as somebody's doing sent seven followers
        // back and focused the tab just left, which fired another switch.
        let c = catch(false, Settling::InFlight);
        assert_eq!(c, Catch::MidSwitch);
        assert!(!c.brings_followers(), "the followers are already at the real destination");
        assert!(!c.moves_the_tab(), "this is the bounce");
    }

    #[test]
    fn a_second_run_while_one_is_in_flight_does_nothing_at_all() {
        // Measured: two concurrent runs moving seven windows apiece, at the
        // same second, in opposite directions to the switch that caused them.
        for settling in [Settling::Ours, Settling::InFlight, Settling::Settled] {
            let c = catch(true, settling);
            assert_eq!(c, Catch::Busy, "{settling:?}");
            assert!(!c.brings_followers());
            assert!(!c.moves_the_tab());
        }
    }

    #[test]
    fn a_change_nobody_here_caused_is_chased() {
        // Clicking a window, the menu bar, a keybinding: the followers come
        // along and herdr's tab comes with them.
        let c = catch(false, Settling::Settled);
        assert_eq!(c, Catch::Chase);
        assert!(c.brings_followers());
        assert!(c.moves_the_tab());
    }

    #[test]
    fn a_tab_clicked_during_the_switch_gets_the_last_word() {
        // Observed: two taps 400ms apart left herdr on t/slingr and AeroSpace
        // on t-video, because the lock turned the second away and nothing
        // looked again.
        assert!(look_again("t-video", Some("t-slingr")), "the tab moved on");
        assert!(!look_again("t-video", Some("t-video")), "landed where it was going");
        assert!(!look_again("t-video", None), "no tab focused, nothing to chase");
    }
}
