//! The only module that shells out to `aerospace`.
//!
//! Every call is bounded by a timeout. The AeroSpace server has wedged before
//! under scripted bursts, and an unbounded query means a wedged server takes
//! sling down with it — no dialog, no clue why. A timeout instead degrades to
//! the cached menu and leaves a line in the action log.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::picker::{occupancy_from, parse_window, Occupancy, Window};
use crate::wm::{run_bounded, CallFailure, Synced, WindowManager};

pub const BIN: &str = "/opt/homebrew/bin/aerospace";
/// Generous on purpose. A wedged server never answers, so waiting longer costs
/// one slow menu; a tight timeout on a loaded machine costs a wrong one.
pub const TIMEOUT: Duration = Duration::from_secs(5);
/// Showing a crowded workspace is slow, and abandoning the call would not stop
/// AeroSpace doing the work anyway.
pub const SWITCH_TIMEOUT: Duration = Duration::from_secs(180);

pub const FORMAT: &str =
    "%{window-id}|%{workspace}|%{monitor-id}|%{app-name}|%{app-bundle-id}|%{window-title}";

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
        run_bounded(&self.binary, args, self.timeout)
    }
}

impl WindowManager for AeroSpace {
    fn name(&self) -> &str {
        "AeroSpace"
    }

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
    fn occupancy(&self) -> Option<BTreeMap<String, Occupancy>> {
        // Two fields where there was one, but still a single call: the icon
        // stack costs nothing extra at the AeroSpace end, and this query is on
        // the path of every sling.
        let out =
            self.run(&["list-windows", "--all", "--format", "%{workspace}|%{app-bundle-id}"])?;
        let pairs: Vec<(&str, &str)> = out
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| {
                let (workspace, bundle) = line.split_once('|').unwrap_or((line, ""));
                (workspace.trim(), bundle.trim())
            })
            .collect();
        Some(occupancy_from(pairs.into_iter()))
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

    fn all_workspaces(&self) -> Option<Vec<String>> {
        let out = self.run(&["list-workspaces", "--all"])?;
        Some(out.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
    }

    fn focused_workspace(&self) -> Option<String> {
        let out = self.run(&["list-workspaces", "--focused"])?;
        Some(out.lines().next()?.trim().to_string())
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

    /// Every task written into `persistent-workspaces`, so it exists — and can
    /// be switched to, and appears in AeroSpace's menu bar — before anything
    /// is in it.
    fn sync_workspaces(&self, names: &[String]) -> Synced {
        let text = std::fs::read_to_string(crate::aeroconf::path()).unwrap_or_default();
        if !crate::aeroconf::declares_version_2(&text) {
            return Synced::Refused(
                "~/.aerospace.toml needs `config-version = 2` before persistent-workspaces works".into(),
            );
        }
        match crate::aeroconf::sync(names) {
            Ok(true) => {
                let _ = Command::new(&self.binary).arg("reload-config").status();
                Synced::Written
            }
            Ok(false) => Synced::Unchanged,
            Err(why) => Synced::Refused(format!("{why:#}")),
        }
    }

    /// AeroSpace's own event stream, so the follow list keeps up with a
    /// workspace change from any cause — a keybinding, a click, or sling.
    fn workspace_changes(&self) -> Option<Box<dyn Iterator<Item = String> + Send>> {
        let child = Command::new(&self.binary)
            .args(["subscribe", "focused-workspace-changed", "--no-send-initial"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let out = child.stdout?;
        Some(Box::new(
            BufReader::new(out)
                .lines()
                .map_while(Result::ok)
                .filter_map(|line| crate::watch::workspace_changed(&line)),
        ))
    }

    /// The raw line, so a probe shows exactly what AeroSpace said — or exactly
    /// why it said nothing, which is usually what a probe is run to find out.
    fn probe_focused(&self) -> String {
        match self.try_run(&["list-windows", "--focused", "--format", FORMAT]) {
            Ok(out) if out.is_empty() => "(nothing focused)".to_string(),
            Ok(out) => out,
            Err(why) => format!("FAILED: {why}"),
        }
    }
}
