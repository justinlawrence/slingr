//! The only module that shells out to `aerospace`.
//!
//! Every call is bounded by a timeout. The AeroSpace server has wedged before
//! under scripted bursts, and an unbounded query means a wedged server takes
//! sling down with it — no dialog, no clue why. A timeout instead degrades to
//! the cached menu and leaves a line in the action log.

use std::collections::BTreeMap;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::picker::{parse_window, Window};

pub const BIN: &str = "/opt/homebrew/bin/aerospace";
/// Generous on purpose. A wedged server never answers, so waiting longer costs
/// one slow menu; a tight timeout on a loaded machine costs a wrong one.
pub const TIMEOUT: Duration = Duration::from_secs(5);

pub const FORMAT: &str =
    "%{window-id}|%{workspace}|%{monitor-id}|%{app-name}|%{app-bundle-id}|%{window-title}";

/// What sling needs from a window manager. A trait so the flow can be tested
/// without a running AeroSpace — see `tests/flow.rs`.
pub trait WindowManager {
    fn focused_window(&self) -> Option<Window>;
    /// Every window AeroSpace manages, across all workspaces.
    fn all_windows(&self) -> Option<Vec<Window>>;
    /// How many windows each workspace holds; `None` if the query failed.
    /// Workspaces holding nothing are absent rather than zero.
    fn window_counts(&self) -> Option<BTreeMap<String, usize>>;
    /// Focus a window by id, returning whether it actually took.
    fn focus(&self, window_id: &str) -> bool;
    fn move_focused_to(&self, workspace: &str) -> bool;
    /// Bring a workspace to the front.
    fn focus_workspace(&self, workspace: &str) -> bool;
    /// Move a window by id, without focusing it first.
    fn move_window(&self, window_id: &str, workspace: &str) -> bool;
    /// The screen currently being worked on.
    fn focused_monitor(&self) -> Option<String>;
}

/// Why a call produced nothing. Kept apart from `None` so that "AeroSpace is
/// wedged" and "AeroSpace answered, with nothing" stay distinguishable — they
/// call for opposite reactions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallFailure {
    /// The binary could not be started at all.
    Spawn,
    /// It ran and refused.
    Status(Option<i32>),
    /// It never answered. A wedged server looks like this.
    Timeout,
}

impl std::fmt::Display for CallFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallFailure::Spawn => write!(f, "could not start {BIN}"),
            CallFailure::Status(Some(c)) => write!(f, "exited {c}"),
            CallFailure::Status(None) => write!(f, "killed by a signal"),
            CallFailure::Timeout => write!(f, "no answer within {}s", TIMEOUT.as_secs()),
        }
    }
}

pub struct AeroSpace {
    binary: String,
    timeout: Duration,
}

impl Default for AeroSpace {
    fn default() -> Self {
        Self { binary: BIN.to_string(), timeout: TIMEOUT }
    }
}

impl AeroSpace {
    /// Showing a workspace restores every window in it, which can take minutes
    /// for a crowded one. The default 5s is right for a query and far too
    /// short for that.
    pub fn with_timeout(timeout: Duration) -> Self {
        Self { timeout, ..Self::default() }
    }

    fn run(&self, args: &[&str]) -> Option<String> {
        self.try_run(args).ok()
    }

    /// The same call, keeping the reason it failed.
    pub fn try_run(&self, args: &[&str]) -> Result<String, CallFailure> {
        let child = match Command::new(&self.binary)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return Err(CallFailure::Spawn),
        };
        let pid = child.id();

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(child.wait_with_output());
        });

        match rx.recv_timeout(self.timeout) {
            Ok(Ok(out)) if out.status.success() => {
                Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
            }
            Ok(Ok(out)) => Err(CallFailure::Status(out.status.code())),
            Ok(Err(_)) => Err(CallFailure::Spawn),
            Err(_) => {
                // Deliberately not killed. A stuck client is cheap — it exits
                // if the server ever answers, and `pkill -f bin/aerospace`
                // clears any that pile up. Signalling a window-manager client
                // mid-request is the riskier half of that trade, and whether
                // it can itself wedge the server has not been established.
                let _ = pid;
                Err(CallFailure::Timeout)
            }
        }
    }
}

impl WindowManager for AeroSpace {
    fn focused_window(&self) -> Option<Window> {
        let out = self.run(&["list-windows", "--focused", "--format", FORMAT])?;
        parse_window(out.lines().next()?)
    }

    fn all_windows(&self) -> Option<Vec<Window>> {
        let out = self.run(&["list-windows", "--all", "--format", FORMAT])?;
        Some(out.lines().filter_map(parse_window).collect())
    }

    /// Asked of the windows rather than of `list-workspaces --all`, which also
    /// reports whichever workspace is active on each monitor even when it is
    /// empty. Counting an empty workspace as occupied would be a lie, and this
    /// costs the same one query.
    fn window_counts(&self) -> Option<BTreeMap<String, usize>> {
        let out = self.run(&["list-windows", "--all", "--format", "%{workspace}"])?;
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for line in out.lines() {
            let name = line.trim();
            if !name.is_empty() {
                *counts.entry(name.to_string()).or_default() += 1;
            }
        }
        Some(counts)
    }

    /// `focus --window-id` exits 0 even when it fails — a minimised window
    /// reports success while focus lands somewhere else entirely. Only the
    /// read-back tells the truth.
    fn focus(&self, window_id: &str) -> bool {
        self.run(&["focus", "--window-id", window_id]);
        self.run(&["list-windows", "--focused", "--format", "%{window-id}"])
            .as_deref()
            == Some(window_id)
    }

    /// There is no way to move a window by id. `move-node-to-workspace` takes
    /// only a workspace name and acts on whatever is focused when it runs, so
    /// the caller must have focused the intended window first — and checked.
    fn move_focused_to(&self, workspace: &str) -> bool {
        self.run(&["move-node-to-workspace", workspace]).is_some()
    }

    fn focus_workspace(&self, workspace: &str) -> bool {
        self.run(&["workspace", workspace]).is_some()
    }

    fn focused_monitor(&self) -> Option<String> {
        let out = self.run(&["list-monitors", "--focused", "--format", "%{monitor-id}"])?;
        Some(out.lines().next()?.trim().to_string())
    }

    /// The whole reason this is cheap. Focusing a window drags the view to its
    /// workspace, and AeroSpace shows a workspace by restoring every window in
    /// it. Naming the window instead means no focus change, no view change and
    /// no restore — about 0.07s rather than tens of seconds.
    ///
    /// It also works on windows that cannot take focus at all, such as
    /// minimised ones. `--` guards a workspace name that starts with a dash.
    fn move_window(&self, window_id: &str, workspace: &str) -> bool {
        self.run(&["move-node-to-workspace", "--window-id", window_id, "--", workspace])
            .is_some()
    }
}
