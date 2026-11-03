//! What slingr asks of a window manager, and which one it asks.
//!
//! AeroSpace was the only answer until macOS's own Spaces were tried as one —
//! see `docs/SPACES.md`. The flows in `app.rs` are written against this trait
//! and nothing else, so a backend is one implementation away, and the fakes in
//! `tests/flow.rs` stand in for every one of them.

use std::collections::BTreeMap;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde::Deserialize;

use crate::aerospace::AeroSpace;
use crate::picker::{Occupancy, Window};
use crate::spaces::Spaces;

/// What sling needs from a window manager. A trait so the flow can be tested
/// without a running one — see `tests/flow.rs`.
pub trait WindowManager {
    fn focused_window(&self) -> Option<Window>;
    /// Every window it manages, across all workspaces.
    fn all_windows(&self) -> Option<Vec<Window>>;
    /// What each workspace holds — how many windows, and which applications
    /// they belong to; `None` if the query failed. Workspaces holding nothing
    /// are absent rather than zero.
    fn occupancy(&self) -> Option<BTreeMap<String, Occupancy>>;
    /// Focus a window by id, returning whether it actually took.
    fn focus(&self, window_id: &str) -> bool;
    fn move_focused_to(&self, workspace: &str) -> bool;
    /// Bring a workspace to the front.
    fn focus_workspace(&self, workspace: &str) -> bool;
    /// Move a window by id, without focusing it first.
    fn move_window(&self, window_id: &str, workspace: &str) -> bool;
    /// The screen currently being worked on.
    fn focused_monitor(&self) -> Option<String>;
    /// The workspace in front. Asked directly rather than derived from the
    /// focused window, because an empty workspace has no focused window and
    /// deriving it there yields nothing at all.
    fn focused_workspace(&self) -> Option<String>;
    /// Every workspace it knows about, empty ones included. With AeroSpace's
    /// `persistent-workspaces` set this is the whole task list, which is why
    /// sling no longer keeps one of its own.
    fn all_workspaces(&self) -> Option<Vec<String>>;

    /// Who is answering, for the rare message that has to name it.
    fn name(&self) -> &str {
        "the window manager"
    }

    /// Whether it can put a window somewhere by itself.
    ///
    /// AeroSpace can. macOS Spaces cannot: with SIP on, SkyLight refuses to
    /// move a window that belongs to another process, by every route tried —
    /// see `docs/SPACES.md`. A backend that cannot is never asked to sling,
    /// its board offers no drags, and nothing is dragged along behind you.
    fn moves_windows(&self) -> bool {
        true
    }

    /// Make every named task exist before anything is in it.
    fn sync_workspaces(&self, _names: &[String]) -> Synced {
        Synced::Unchanged
    }

    /// Workspace changes as they happen, for the resident `watch`. `None`
    /// where something else reports them.
    fn workspace_changes(&self) -> Option<Box<dyn Iterator<Item = String> + Send>> {
        None
    }

    /// One line on what is focused, or exactly why that could not be read.
    fn probe_focused(&self) -> String {
        match self.focused_window() {
            Some(w) => format!("{}|{}|{}|{}|{}", w.id, w.workspace, w.monitor, w.app, w.title),
            None => "(nothing focused)".to_string(),
        }
    }
}

/// What making every task exist came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Synced {
    Written,
    Unchanged,
    /// It would not, and this says why in words a person can act on.
    Refused(String),
}

/// Which window manager a task lives in, as named in `workspaces.toml`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    #[default]
    AeroSpace,
    /// macOS Spaces, one per task. An experiment — see `docs/SPACES.md`.
    Spaces,
}

/// How long a call may take before giving up on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Patience {
    /// A question, which should answer at once.
    Query,
    /// A switch. Showing a crowded AeroSpace workspace restores every window
    /// in it, and abandoning the call would not stop the work anyway.
    Switch,
}

/// The backend `workspaces.toml` names, unless `SLINGR_BACKEND` says otherwise
/// — so one can be tried from a terminal without editing the file every hook
/// reads.
///
/// AeroSpace when nothing names one, and also when the file cannot be read:
/// the hooks run on every tab switch, and a typo there should leave them doing
/// what they always did rather than doing nothing.
pub fn configured() -> Backend {
    if let Some(named) = std::env::var("SLINGR_BACKEND").ok().and_then(|v| parse(&v)) {
        return named;
    }
    crate::config::Config::load().map(|c| c.backend).unwrap_or_default()
}

fn parse(name: &str) -> Option<Backend> {
    toml::Value::String(name.trim().to_string()).try_into().ok()
}

pub fn open(patience: Patience) -> Box<dyn WindowManager> {
    open_for(configured(), patience)
}

pub fn open_for(backend: Backend, patience: Patience) -> Box<dyn WindowManager> {
    match (backend, patience) {
        (Backend::AeroSpace, Patience::Query) => Box::new(AeroSpace::default()),
        (Backend::AeroSpace, Patience::Switch) => {
            Box::new(AeroSpace::with_timeout(crate::aerospace::SWITCH_TIMEOUT))
        }
        // A Space switch is the same short call however much the Space holds.
        (Backend::Spaces, _) => Box::new(Spaces::default()),
    }
}

/// Why a call produced nothing. Kept apart from `None` so that "the window
/// manager is wedged" and "it answered, with nothing" stay distinguishable —
/// they call for opposite reactions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallFailure {
    /// The binary could not be started at all.
    Spawn(String),
    /// It ran and refused.
    Status(Option<i32>),
    /// It never answered. A wedged server looks like this.
    Timeout(Duration),
}

impl std::fmt::Display for CallFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallFailure::Spawn(binary) => write!(f, "could not start {binary}"),
            CallFailure::Status(Some(c)) => write!(f, "exited {c}"),
            CallFailure::Status(None) => write!(f, "killed by a signal"),
            CallFailure::Timeout(after) => write!(f, "no answer within {}s", after.as_secs()),
        }
    }
}

/// Run a command and wait at most `timeout` for its answer.
///
/// An unbounded query means a wedged server takes sling down with it — no
/// dialog, no clue why. A timeout instead degrades to the cached menu and
/// leaves a line in the action log.
pub fn run_bounded(binary: &str, args: &[&str], timeout: Duration) -> Result<String, CallFailure> {
    let child = match Command::new(binary)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return Err(CallFailure::Spawn(binary.to_string())),
    };

    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });

    match rx.recv_timeout(timeout) {
        Ok(Ok(out)) if out.status.success() => Ok(String::from_utf8_lossy(&out.stdout).trim().to_string()),
        Ok(Ok(out)) => Err(CallFailure::Status(out.status.code())),
        Ok(Err(_)) => Err(CallFailure::Spawn(binary.to_string())),
        // Deliberately not killed. A stuck AeroSpace client is cheap — it
        // exits if the server ever answers, and `pkill -f bin/aerospace`
        // clears any that pile up. Signalling a window-manager client
        // mid-request is the riskier half of that trade, and whether it can
        // itself wedge the server has not been established.
        Err(_) => Err(CallFailure::Timeout(timeout)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Named {
        #[serde(default)]
        backend: Backend,
    }

    #[test]
    fn the_backend_is_spelled_in_lower_case_and_defaults_to_aerospace() {
        let read = |text: &str| toml::from_str::<Named>(text).expect("parses").backend;
        assert_eq!(read(""), Backend::AeroSpace, "a config that names none keeps the old behaviour");
        assert_eq!(read("backend = \"aerospace\""), Backend::AeroSpace);
        assert_eq!(read("backend = \"spaces\""), Backend::Spaces);
        assert!(toml::from_str::<Named>("backend = \"Spaces\"").is_err());
    }

    #[test]
    fn the_environment_names_a_backend_the_same_way_the_file_does() {
        assert_eq!(parse("spaces"), Some(Backend::Spaces));
        assert_eq!(parse(" aerospace\n"), Some(Backend::AeroSpace));
        assert_eq!(parse("yabai"), None, "an unknown name falls through to the file");
    }

    #[test]
    fn a_missing_binary_says_which_one() {
        let failed = run_bounded("/nonexistent/slingr-test", &[], Duration::from_secs(1));
        assert_eq!(failed, Err(CallFailure::Spawn("/nonexistent/slingr-test".into())));
        assert_eq!(failed.unwrap_err().to_string(), "could not start /nonexistent/slingr-test");
    }
}
