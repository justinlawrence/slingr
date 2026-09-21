use std::io::BufRead;
use std::io::BufReader;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::{Parser, Subcommand};
use serde_json::json;

use slingr::aerospace::{self, AeroSpace};
use slingr::app::{self, Batch, Outcome, Run, Session};
use slingr::picker::Window;
use slingr::config::{self, Config};
use slingr::dialog::{Prompt, SystemEvents};
use slingr::panel::Panel;
use slingr::store::{self, ActionLog, FollowList, Layout, NameCache, Pins, Placed};

#[derive(Parser)]
#[command(name = "slingr", about = "Throw a window at a task.", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Bring the follow list to the workspace in front, now. `watch` does
    /// this automatically on every workspace change.
    Follow,
    /// List the windows that come along to every task.
    Following {
        /// Forget the ones whose window no longer exists.
        #[arg(long)]
        prune: bool,
    },
    /// Open the picker on the task list, to go to one rather than send a
    /// window to one.
    Jump,
    /// Go to the workspace matching herdr's focused tab, once.
    ///
    /// Made for herdr's `tab.focused` plugin hook. Does the same as a single
    /// beat of `watch`, without anything staying resident.
    Goto,
    /// Follow herdr's focused tab into the matching workspace, staying
    /// resident. Only needed where the callbacks cannot be installed.
    Watch {
        /// Say what would happen without switching anything.
        #[arg(long)]
        dry_run: bool,
        /// Poll instead of subscribing, for when the socket is unavailable.
        #[arg(long)]
        poll: bool,
        /// Poll interval in milliseconds. Only used with --poll.
        #[arg(long, default_value_t = 400)]
        interval: u64,
        /// How long a tab must stay focused before following it, in
        /// milliseconds. Stops a flick through tabs queueing a switch each.
        #[arg(long, default_value_t = 250)]
        settle: u64,
    },
    /// Write every known task into AeroSpace's `persistent-workspaces`, so a
    /// task exists before anything is in it.
    Sync,
    /// Record where every window is, so a restart can be undone.
    Snapshot,
    /// Put every window back where the last snapshot had it.
    Restore {
        /// List what would move, changing nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Summarise the action log — where windows actually go.
    Stats {
        /// How many recent slings to list.
        #[arg(long, default_value_t = 15)]
        recent: usize,
    },
    /// Print the paths sling reads and writes.
    Paths,
    /// Ask AeroSpace what it sees, and change nothing.
    Probe,
}

/// Printing that stops quietly when the reader goes away, as in
/// `sling stats | head`. Rust ignores SIGPIPE, so a plain `println!` panics on
/// a closed pipe instead of ending the program.
macro_rules! say {
    ($($arg:tt)*) => {{
        use std::io::Write;
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    }};
}

fn main() -> Result<()> {
    match Cli::parse().command {
        None => sling(),
        Some(Cmd::Watch { dry_run, poll, interval, settle }) => {
            watch(dry_run, poll, interval, settle)
        }
        Some(Cmd::Sync) => sync_workspaces(true),
        Some(Cmd::Snapshot) => { snapshot()?; Ok(()) }
        Some(Cmd::Restore { dry_run }) => restore(dry_run),
        Some(Cmd::Jump) => sling_in(app::Mode::Jump),
        Some(Cmd::Goto) => goto_now(),
        Some(Cmd::Follow) => follow_now(),
        Some(Cmd::Following { prune }) => list_following(prune),
        Some(Cmd::Stats { recent }) => stats(recent),
        Some(Cmd::Probe) => probe(),
        Some(Cmd::Paths) => {
            say!("panel   {}{}", slingr::panel::binary().display(),
                 if slingr::panel::available() { "" } else { "   (missing — falls back to the dialog)" });
            say!("config  {}", config::config_path().display());
            say!("state   {}", store::state_dir().display());
            say!("log     {}", ActionLog::default().path().display());
            Ok(())
        }
    }
}

/// One log line. Both flows write the same shape, so `sling stats` needs no
/// special case for a batch.
fn log_entry(outcome: &Outcome, window: Option<&Window>) -> serde_json::Value {
    let mut entry = json!({
        "at": store::iso8601(store::now_unix()),
        "outcome": outcome.kind(),
    });
    if let Some(w) = window {
        entry["from"] = json!(w.workspace);
        entry["window"] = json!({ "id": w.id, "app": w.app, "title": w.title });
    }
    match outcome {
        Outcome::Moved { to, created } => {
            entry["to"] = json!(to);
            entry["created"] = json!(created);
        }
        Outcome::MoveFailed { to } => entry["to"] = json!(to),
        Outcome::EmptyName { raw } => entry["raw"] = json!(raw),
        _ => {}
    }
    entry
}

fn sling() -> Result<()> {
    sling_in(app::Mode::One)
}

fn sling_in(start: app::Mode) -> Result<()> {
    let mut cfg = Config::load()?;
    // A task is usually a herdr tab, so the tabs are offered whether or not
    // anyone has written them into the config.
    for task in slingr::herdr::tasks() {
        if !cfg.known.contains(&task) {
            cfg.known.push(task);
        }
    }
    let mut cache = NameCache::load();
    let mut follow = FollowList::load();
    let mut pins = Pins::load();
    // The panel when it is there, the AppleScript dialog when it is not, so a
    // missing build degrades to something that still works.
    let prompt: Box<dyn Prompt> = if slingr::panel::available() {
        Box::new(Panel)
    } else {
        Box::new(SystemEvents)
    };

    // Pinning changes the order of the list, so it answers by asking again.
    // Bounded for the same reason the mode switch is: a prompt that only ever
    // pins must not spin.
    for _ in 0..24 {
        let session = app::run_session_from(
            start,
            &AeroSpace::default(),
            prompt.as_ref(),
            &cfg,
            &cache.workspaces,
            &follow.ids(),
            &pins.tasks,
        );
        match session {
            Session::Single(run) => {
                if let Outcome::PinToggled { workspace } = &run.outcome {
                    pins.toggle(workspace);
                    pins.save()?;
                    continue;
                }
                return record_one(run, &mut cache, &mut follow);
            }
            Session::Batch(batch) => return record_many(batch, &mut cache, &mut follow),
        }
    }
    Ok(())
}

/// Toggle membership of the follow list for whatever the outcome says.
fn apply_follow(outcome: &Outcome, window: Option<&Window>, follow: &mut FollowList) -> Result<()> {
    let (Some(w), true) = (
        window,
        matches!(outcome, Outcome::Following | Outcome::Unfollowing),
    ) else {
        return Ok(());
    };
    follow.toggle(&w.id, &w.app, &w.title);
    follow.save()
}

/// One at a time. herdr has been reported to emit focus events in bursts, and
/// a hook that spawns a process per event must not pile them up.
fn only_one(name: &str) -> Option<std::fs::File> {
    use std::io::Write;
    let path = store::state_dir().join(format!("{name}.lock"));
    std::fs::create_dir_all(store::state_dir()).ok()?;
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(pid) = text.trim().parse::<i32>() {
            // Signal 0 asks whether the process exists without disturbing it.
            let alive = unsafe { libc_kill(pid, 0) } == 0;
            if alive {
                return None;
            }
        }
    }
    let mut file = std::fs::File::create(&path).ok()?;
    let _ = write!(file, "{}", std::process::id());
    Some(file)
}

extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

fn goto_now() -> Result<()> {
    use slingr::aerospace::WindowManager;
    use slingr::watch::{decide, Action};

    let Some(_held) = only_one("goto") else {
        return Ok(());
    };
    let aero = AeroSpace::with_timeout(SWITCH_TIMEOUT);
    let tab = slingr::herdr::focused();
    let here = aero.focused_window().map(|w| w.workspace).unwrap_or_default();
    let counts = aero.window_counts().unwrap_or_default();

    // No settling to wait for: the hook fires once per actual focus change,
    // so the tab has already stopped moving by the time we are called.
    match decide(tab.as_deref(), tab.as_deref(), &here, &counts) {
        Action::Switch(target) => {
            aero.focus_workspace(&target);
            say!("{here} -> {target}");
        }
        Action::Hold(why) => {
            if let Some(t) = &tab {
                say!("holding: {why} ({t})");
            }
        }
    }
    Ok(())
}

fn follow_now() -> Result<()> {
    use slingr::aerospace::WindowManager;

    let aero = AeroSpace::default();
    let follow = FollowList::load();
    let Some(here) = aero.focused_window().map(|w| w.workspace) else {
        say!("AeroSpace did not answer");
        return Ok(());
    };
    // Only the screen being worked on. Clicking a window on another display
    // must not pull everything across to it.
    let on = aero.focused_monitor();
    let brought = app::follow_to(&aero, &follow.ids(), &here, "", None, on.as_deref());
    if !brought.is_empty() {
        say!("brought {} to {here}", brought.len());
    }

    // The other half of the pairing. A task is a herdr tab and a workspace, so
    // arriving at the workspace should bring the tab with it — whether you got
    // here by keybinding, by the menu bar, or by clicking a window.
    //
    // This cannot chase its own tail: focusing a tab that is already focused
    // is skipped, and were it not, the return trip finds the workspace already
    // correct and stops there.
    if let Some(tab) = slingr::herdr::tab_named(&here) {
        if !tab.focused && slingr::herdr::focus_tab(&tab.id) {
            say!("herdr -> {here}");
        }
    }

    let _ = snapshot();
    Ok(())
}

fn list_following(prune: bool) -> Result<()> {
    use slingr::aerospace::WindowManager;

    let mut follow = FollowList::load();
    if follow.windows.is_empty() {
        say!("nothing follows you yet — pick \"all workspaces\" in the picker");
        return Ok(());
    }

    // Window ids do not survive the application restarting, so an entry can
    // outlive the window it named. Such an entry is skipped anyway, but a list
    // that does not say so is worse than no list.
    let live: Vec<String> = AeroSpace::default()
        .all_windows()
        .map(|all| all.into_iter().map(|w| w.id).collect())
        .unwrap_or_default();

    let mut stale = 0;
    for w in &follow.windows {
        let gone = !live.is_empty() && !live.contains(&w.id);
        if gone {
            stale += 1;
        }
        say!(
            "  {} [{}] {} — {}",
            if gone { "gone" } else { "    " },
            w.id,
            w.app,
            w.title
        );
    }

    if stale > 0 && prune {
        follow.windows.retain(|w| live.contains(&w.id));
        follow.save()?;
        say!("\nforgot {stale} whose window no longer exists");
    } else if stale > 0 {
        say!("\n{stale} point at windows that are gone — `slingr following --prune` forgets them");
    }
    Ok(())
}

fn record_one(run: Run, cache: &mut NameCache, follow: &mut FollowList) -> Result<()> {
    apply_follow(&run.outcome, run.window.as_ref(), follow)?;
    for (w, outcome) in &run.brought {
        let mut entry = log_entry(outcome, Some(w));
        entry["followed"] = json!(true);
        ActionLog::default().append(&entry)?;
    }
    let mut entry = log_entry(&run.outcome, run.window.as_ref());
    if run.counts.is_none() {
        entry["aerospace_unavailable"] = json!(true);
    }
    ActionLog::default().append(&entry)?;

    // A task invented just now has to exist before it can be switched to, so
    // AeroSpace is told about it rather than sling remembering it.
    if let Outcome::Moved { created: true, .. } = &run.outcome {
        let _ = sync_workspaces(false);
    }
    let _ = cache;
    Ok(())
}

fn record_many(batch: Batch, cache: &mut NameCache, follow: &mut FollowList) -> Result<()> {
    let log = ActionLog::default();
    for (w, outcome) in &batch.results {
        apply_follow(outcome, Some(w), follow)?;
    }

    if let Some(outcome) = &batch.aborted {
        log.append(&log_entry(outcome, None))?;
        say!("nothing moved ({})", outcome.kind());
        return Ok(());
    }

    let size = batch.results.len();
    for (window, outcome) in &batch.results {
        let mut entry = log_entry(outcome, Some(window));
        entry["batch"] = json!(size);
        log.append(&entry)?;
    }

    let moved = batch.moved();
    let target = batch.target.clone().unwrap_or_default();
    say!("moved {moved} of {size} to {target}");
    for (window, outcome) in &batch.results {
        if !matches!(outcome, Outcome::Moved { .. }) {
            say!("  skipped [{}] {} — {}", window.id, window.label(), outcome.kind());
        }
    }

    if batch.results.iter().any(|(_, o)| matches!(o, Outcome::Moved { created: true, .. })) {
        let _ = sync_workspaces(false);
    }
    let _ = cache;
    Ok(())
}

/// Showing a crowded workspace is slow, and abandoning the call would not stop
/// AeroSpace doing the work anyway.
const SWITCH_TIMEOUT: Duration = Duration::from_secs(180);

fn watch(dry_run: bool, poll: bool, interval_ms: u64, settle_ms: u64) -> Result<()> {
    let aero = AeroSpace::with_timeout(SWITCH_TIMEOUT);
    let settle = Duration::from_millis(settle_ms);

    if poll {
        return watch_by_polling(&aero, dry_run, Duration::from_millis(interval_ms.max(100)));
    }

    let stream = match slingr::herdr::subscribe(&["tab.focused"]) {
        Ok(s) => s,
        Err(why) => {
            say!("could not subscribe at {}: {why}", slingr::herdr::socket_path().display());
            say!("try: slingr watch --poll");
            return Ok(());
        }
    };
    say!("watching herdr{}  (ctrl-c to stop)", if dry_run { ", dry run" } else { "" });

    // Read on its own thread and coalesce. herdr can emit phantom focus
    // events in bursts (~29/s has been reported on sessions with several
    // agents producing output), and one workspace switch per event would be
    // ruinous — each is a full window restore. Collapsing a burst to a single
    // action also makes the watcher correct rather than merely fast: the live
    // focused tab is read afterwards, so a discarded event says nothing new.
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            if slingr::watch::is_tab_focused(&line) && tx.send(()).is_err() {
                break;
            }
        }
    });

    // AeroSpace's own events, so the follow list keeps up with a workspace
    // change from any cause — a keybinding, a click, or sling itself. Moving a
    // window by id changes neither focus nor workspace, so this cannot feed
    // itself: bringing followers raises no further event.
    if !dry_run {
        thread::spawn(move || {
            let Ok(child) = std::process::Command::new(slingr::aerospace::BIN)
                .args(["subscribe", "focused-workspace-changed", "--no-send-initial"])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()
            else {
                return;
            };
            let Some(out) = child.stdout else { return };
            use slingr::aerospace::WindowManager;
            let aero = AeroSpace::default();
            let mut last = String::new();
            for line in BufReader::new(out).lines() {
                let Ok(line) = line else { break };
                let Some(workspace) = slingr::watch::workspace_changed(&line) else {
                    continue;
                };
                if workspace == last {
                    continue;
                }
                last = workspace.clone();
                let follow = FollowList::load();
                if !follow.windows.is_empty() {
                    let on = aero.focused_monitor();
                    app::follow_to(&aero, &follow.ids(), &workspace, "", None, on.as_deref());
                }
                let _ = snapshot();
            }
        });
    }

    while rx.recv().is_ok() {
        while rx.try_recv().is_ok() {}
        // The event carries tab_id, not the label, and a rename would make a
        // cached mapping wrong. Asking costs about 7ms and is always right.
        let before = slingr::herdr::focused();
        thread::sleep(settle);
        while rx.try_recv().is_ok() {}
        act_on(&aero, slingr::herdr::focused(), before, dry_run);
    }
    say!("herdr closed the connection");
    Ok(())
}

fn watch_by_polling(aero: &AeroSpace, dry_run: bool, wait: Duration) -> Result<()> {
    say!(
        "polling herdr every {}ms{}  (ctrl-c to stop)",
        wait.as_millis(),
        if dry_run { ", dry run" } else { "" }
    );
    let mut settled: Option<String> = None;
    loop {
        let tab = slingr::herdr::focused();
        act_on(aero, tab.clone(), settled, dry_run);
        settled = tab;
        thread::sleep(wait);
    }
}

/// Decide and, if it says so, switch. `settled` is the tab seen a moment ago;
/// following only a tab that has stopped moving is what stops a flick through
/// tabs queueing a workspace switch for each one passed.
///
/// Every decision is written to `watch.jsonl` with timings and the state it
/// was made from, including where AeroSpace ended up a moment later. That last
/// field is the one worth having: a switch that is immediately undone looks
/// identical from outside to a switch that never happened.
fn act_on(aero: &AeroSpace, tab: Option<String>, settled: Option<String>, dry_run: bool) {
    use slingr::aerospace::WindowManager;
    use slingr::watch::{decide, Action};

    let began = Instant::now();
    let focused = aero.focused_window();
    let here = focused.as_ref().map(|w| w.workspace.clone()).unwrap_or_default();
    let counts = aero.window_counts().unwrap_or_default();
    let read_ms = began.elapsed().as_millis();

    let mut entry = json!({
        "at": store::iso8601(store::now_unix()),
        "tab": tab,
        "settled": settled,
        "here": here,
        "focused_window": focused.as_ref().map(|w| json!({
            "id": w.id, "app": w.app, "workspace": w.workspace,
        })),
        "counts": counts,
        "read_ms": read_ms,
    });

    match decide(tab.as_deref(), settled.as_deref(), &here, &counts) {
        Action::Switch(target) => {
            let follow = FollowList::load();
            entry["target"] = json!(target);
            if dry_run {
                say!("would switch {here} -> {target} (bringing {})", follow.windows.len());
                entry["action"] = json!("would_switch");
            } else {
                say!("{here} -> {target}");
                // Bring the follow list across FIRST. Moving a window out of
                // the workspace on screen is cheap; the switch then restores
                // everything, them included, in one go. Moving them in
                // afterwards would pay the expensive direction a second time.
                //
                // This is what keeps the herdr terminal on screen. Without it,
                // following a tab hides the very window being driven from —
                // AeroSpace hides everything not in the active workspace, and
                // the terminal lives in exactly one.
                let t0 = Instant::now();
                let on = aero.focused_monitor();
                let brought =
                    app::follow_to(aero, &follow.ids(), &target, "", Some(&here), on.as_deref());
                let follow_ms = t0.elapsed().as_millis();
                if follow_ms > 8000 {
                    say!(
                        "  !! {}s spent bringing {} followers ({} workspace restores)",
                        follow_ms / 1000,
                        brought.len(),
                        app::restores_for(brought.len())
                    );
                    say!("     each follower past the first costs two more — `slingr following`");
                }

                let t1 = Instant::now();
                let ok = aero.focus_workspace(&target);
                let switch_ms = t1.elapsed().as_millis();

                // Did it stay? A window left behind in the old workspace can
                // pull focus straight back, which reads as a flash and no
                // apparent change.
                thread::sleep(Duration::from_millis(1200));
                let after = aero.focused_window();

                entry["action"] = json!("switch");
                entry["switch_ok"] = json!(ok);
                entry["follow_ms"] = json!(follow_ms);
                entry["switch_ms"] = json!(switch_ms);
                entry["brought"] = json!(brought
                    .iter()
                    .map(|(w, o)| json!({"id": w.id, "app": w.app, "outcome": o.kind()}))
                    .collect::<Vec<_>>());
                entry["settled_on"] = json!(after.as_ref().map(|w| w.workspace.clone()));
                entry["settled_focus"] = json!(after.as_ref().map(|w| json!({
                    "id": w.id, "app": w.app,
                })));

                if after.as_ref().map(|w| w.workspace.as_str()) != Some(target.as_str()) {
                    let landed = after.map(|w| w.workspace).unwrap_or_default();
                    say!("  !! bounced back to {landed} — something there took focus");
                    entry["bounced"] = json!(true);
                }
            }
        }
        Action::Hold(why) => {
            entry["action"] = json!("hold");
            entry["why"] = json!(why);
            if let Some(t) = &tab {
                say!("holding: {why} ({t})");
            }
        }
    }
    let _ = ActionLog::watch().append(&entry);
}

/// Teach AeroSpace every task we know about, from every source at once.
///
/// This is what lets a brand-new task be switched to, appear in AeroSpace's
/// menu bar, and be offered by name — without sling keeping a list of its own.
fn sync_workspaces(loud: bool) -> Result<()> {
    use slingr::aerospace::WindowManager;

    let cfg = Config::load()?;
    let aero = AeroSpace::default();

    let mut names: Vec<String> = Vec::new();
    let mut add = |candidates: Vec<String>| {
        for name in candidates {
            if !name.is_empty() && !names.contains(&name) {
                names.push(name);
            }
        }
    };
    add(slingr::herdr::tasks());
    add(cfg.known.clone());
    add(aero.all_workspaces().unwrap_or_default());
    // Whatever the old remembered-names file still holds, so nothing that was
    // invented before this existed is lost.
    add(store::NameCache::load().workspaces);
    names.sort();

    let text = std::fs::read_to_string(slingr::aeroconf::path()).unwrap_or_default();
    if !slingr::aeroconf::declares_version_2(&text) {
        say!("~/.aerospace.toml needs `config-version = 2` before persistent-workspaces works");
        return Ok(());
    }

    let changed = slingr::aeroconf::sync(&names)?;
    if changed {
        let _ = std::process::Command::new(slingr::aerospace::BIN)
            .arg("reload-config")
            .status();
    }
    if loud {
        say!(
            "{} tasks {}",
            names.len(),
            if changed { "written and reloaded" } else { "already up to date" }
        );
    }
    Ok(())
}

/// Write down where everything is. Cheap enough to do after every change.
fn snapshot() -> Result<usize> {
    use slingr::aerospace::WindowManager;

    let Some(windows) = AeroSpace::default().all_windows() else {
        return Ok(0);
    };
    let layout = Layout {
        at: store::iso8601(store::now_unix()),
        windows: windows
            .into_iter()
            .map(|w| Placed { id: w.id, workspace: w.workspace, app: w.app, title: w.title })
            .collect(),
    };
    let n = layout.windows.len();
    layout.save()?;
    Ok(n)
}

fn restore(dry_run: bool) -> Result<()> {
    use slingr::aerospace::WindowManager;

    let layout = Layout::load();
    if layout.windows.is_empty() {
        say!("no snapshot yet — run `slingr snapshot`");
        return Ok(());
    }
    let aero = AeroSpace::default();
    let Some(live) = aero.all_windows() else {
        say!("AeroSpace did not answer");
        return Ok(());
    };
    let here: Vec<(String, String, String)> = live
        .iter()
        .map(|w| (w.id.clone(), w.app.clone(), w.title.clone()))
        .collect();
    let now: std::collections::BTreeMap<&str, &str> =
        live.iter().map(|w| (w.id.as_str(), w.workspace.as_str())).collect();

    let began = Instant::now();
    let (mut moved, mut failed, mut already) = (0, 0, 0);
    for (id, workspace) in layout.resolve(&here) {
        if now.get(id) == Some(&workspace) {
            already += 1;
            continue;
        }
        if dry_run {
            say!("would move {id} -> {workspace}");
            moved += 1;
        } else if aero.move_window(id, workspace) {
            moved += 1;
        } else {
            failed += 1;
        }
    }
    let missing = layout.windows.len() - moved - failed - already;
    say!(
        "snapshot from {}: {moved} moved, {already} already right, {failed} failed, {missing} gone ({:.1}s)",
        layout.at,
        began.elapsed().as_secs_f32()
    );
    Ok(())
}

/// Read-only: the two queries a sling starts with, and nothing else.
fn probe() -> Result<()> {
    use slingr::aerospace::WindowManager;

    let aero = AeroSpace::default();
    match aero.try_run(&["list-windows", "--focused", "--format", aerospace::FORMAT]) {
        Ok(out) if out.is_empty() => say!("focused  (nothing focused)"),
        Ok(out) => say!("focused  {out}"),
        Err(why) => say!("focused  FAILED: {why}"),
    }
    match aero.window_counts() {
        Some(counts) => {
            let shown: Vec<String> =
                counts.iter().map(|(w, n)| format!("{w} ({n})")).collect();
            say!("occupied {}", shown.join(", "));
        }
        None => say!("occupied FAILED"),
    }
    let cache = NameCache::load();
    say!("herdr    {}", slingr::herdr::tasks().join(", "));
    say!("known    {}", Config::load()?.known.join(", "));
    say!("seen     {}", cache.workspaces.join(", "));
    Ok(())
}

fn stats(recent: usize) -> Result<()> {
    use std::collections::BTreeMap;

    let path = ActionLog::default().path().clone();
    let Ok(text) = std::fs::read_to_string(&path) else {
        say!("no log yet at {}", path.display());
        return Ok(());
    };
    let entries: Vec<serde_json::Value> =
        text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();

    let mut outcomes: BTreeMap<String, usize> = BTreeMap::new();
    let mut targets: BTreeMap<String, usize> = BTreeMap::new();
    for e in &entries {
        let kind = e["outcome"].as_str().unwrap_or("?").to_string();
        *outcomes.entry(kind).or_default() += 1;
        if let Some(to) = e["to"].as_str() {
            *targets.entry(to.to_string()).or_default() += 1;
        }
    }

    say!("{} slings recorded\n", entries.len());
    say!("outcomes");
    for (kind, n) in &outcomes {
        say!("  {n:>4}  {kind}");
    }

    if !targets.is_empty() {
        let mut ranked: Vec<_> = targets.iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        say!("\nwhere windows went");
        for (to, n) in ranked {
            say!("  {n:>4}  {to}");
        }
    }

    say!("\nlast {recent}");
    for e in entries.iter().rev().take(recent).rev() {
        let at = e["at"].as_str().unwrap_or("");
        let app = e["window"]["app"].as_str().unwrap_or("?");
        let from = e["from"].as_str().unwrap_or("?");
        let to = e["to"].as_str().unwrap_or("-");
        say!("  {at}  {:<16} {app:<16} {from} -> {to}", e["outcome"].as_str().unwrap_or("?"));
    }
    Ok(())
}
