use std::io::BufRead;
use std::io::BufReader;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::{Parser, Subcommand};
use serde_json::json;

use slingr::app::{self, Batch, Outcome, Run, Session};
use slingr::picker::Window;
use slingr::config::{self, Config};
use slingr::dialog::{Prompt, SystemEvents};
use slingr::panel::Panel;
use slingr::store::{self, ActionLog, FollowList, Layout, NameCache, Pins, Placed};
use slingr::wm::{self, Patience, Synced, WindowManager};

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
    /// Open the board: every window on the machine, grouped by the task it is
    /// in, with dragging to move them between tasks.
    Board,
    /// Go to the workspace matching herdr's focused tab, once.
    ///
    /// Made for herdr's `tab.focused` plugin hook. Does the same as a single
    /// beat of `watch`, without anything staying resident.
    Goto,
    /// Say, in a notification that takes you there when clicked, that an
    /// agent needs input or has finished; take it down once the agent moves
    /// on.
    ///
    /// Made for herdr's `pane.agent_status_changed` plugin hook, and reads
    /// the change from `HERDR_PLUGIN_EVENT_JSON`.
    Notify,
    /// Go to the agent a notification was about: what clicking one runs.
    Answer {
        /// The agent's pane, as herdr names it: `w7:p2J`.
        pane: String,
        /// Its tab, gone to instead if the agent has gone.
        #[arg(long)]
        tab: Option<String>,
    },
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
    /// Ask the window manager what it sees, and change nothing.
    Probe,
    /// Print the task in front. What the agent shows in the menu bar, since
    /// macOS has no name for a Space.
    Here,
    /// List every screen's Spaces and the task each one is for.
    Spaces,
    /// Print, as JSON, the task each desktop is for, screen by screen in
    /// Mission Control's order: what the agent writes over Mission Control's
    /// "Desktop N", since macOS will not let a desktop be renamed.
    Labels,
    /// Give the desktop showing on the main screen to a task — how a desktop
    /// that already holds windows becomes one.
    Bind {
        /// The task, named as herdr names it; `t/forms` becomes `t-forms`.
        task: String,
    },
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
        Some(Cmd::Board) => sling_in(app::Mode::Board),
        Some(Cmd::Goto) => goto_now(),
        Some(Cmd::Notify) => notify_from_hook(),
        Some(Cmd::Answer { pane, tab }) => answer(&pane, tab.as_deref()),
        Some(Cmd::Follow) => follow_now(),
        Some(Cmd::Following { prune }) => list_following(prune),
        Some(Cmd::Stats { recent }) => stats(recent),
        Some(Cmd::Probe) => probe(),
        Some(Cmd::Here) => {
            if let Some(here) = wm::open(Patience::Query).focused_workspace() {
                say!("{here}");
            }
            Ok(())
        }
        Some(Cmd::Spaces) => list_spaces(),
        Some(Cmd::Labels) => print_labels(),
        Some(Cmd::Bind { task }) => bind(&task),
        Some(Cmd::Paths) => {
            say!("backend {:?}", wm::configured());
            say!("panel   {}{}", slingr::panel::binary().display(),
                 if slingr::panel::available() { "" } else { "   (missing — falls back to the dialog)" });
            let helper = slingr::panel::beside("slingr-spaces");
            say!("spaces  {}{}", helper.display(),
                 if helper.exists() { "" } else { "   (missing — the Spaces backend cannot answer)" });
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
    let wm = wm::open(Patience::Query);
    // The panel when it is there, the AppleScript dialog when it is not, so a
    // missing build degrades to something that still works.
    let prompt: Box<dyn Prompt> = if slingr::panel::available() {
        Box::new(Panel { drags: wm.moves_windows() })
    } else {
        Box::new(SystemEvents)
    };

    // Pinning changes the order of the list, so it answers by asking again.
    // Bounded for the same reason the mode switch is: a prompt that only ever
    // pins must not spin.
    for _ in 0..24 {
        let session = app::run_session_from(
            start,
            wm.as_ref(),
            prompt.as_ref(),
            &cfg,
            &cache.workspaces,
            app::Following {
                windows: &follow.ids(),
                apps: &follow.bundles(),
                grounded: &follow.grounded_ids(),
            },
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
    let Some(w) = window else { return Ok(()) };
    if app::absorb(follow, outcome, w) {
        follow.save()?;
    }
    Ok(())
}

/// One at a time. herdr has been reported to emit focus events in bursts, and
/// a hook that spawns a process per event must not pile them up.
///
/// The lock removes itself. Leaving the file behind means the next run has to
/// judge a recorded pid, and a pid that has been reused by something unrelated
/// makes the lock look held for ever — the hook then does nothing, silently,
/// which is far worse than the pile-up it was guarding against.
struct OnlyOne(std::path::PathBuf);

impl Drop for OnlyOne {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn only_one(name: &str) -> Option<OnlyOne> {
    use std::io::Write;

    let path = store::state_dir().join(format!("{name}.lock"));
    std::fs::create_dir_all(store::state_dir()).ok()?;

    // Exclusive creation is the whole lock: whoever makes the file holds it.
    let mut file = match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(f) => f,
        Err(_) => {
            // Someone holds it, or something died without cleaning up. Only
            // the second case may be taken over.
            let stale = std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| t.trim().parse::<i32>().ok())
                .map(|pid| unsafe { libc_kill(pid, 0) } != 0)
                .unwrap_or(true);
            if !stale {
                return None;
            }
            std::fs::remove_file(&path).ok()?;
            std::fs::OpenOptions::new().write(true).create_new(true).open(&path).ok()?
        }
    };
    let _ = write!(file, "{}", std::process::id());
    Some(OnlyOne(path))
}

extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

fn goto_now() -> Result<()> {
    use slingr::watch::{decide, Action};

    let Some(_held) = only_one("goto") else {
        // Another one is mid-flight. Recorded rather than silent: a switch
        // that quietly does nothing is the hardest kind to notice.
        note(&json!({ "at": store::iso8601(store::now_unix()), "skipped": "already running" }));
        return Ok(());
    };
    let wm = wm::open(Patience::Switch);
    // At most twice: one look, and one more if the tab moved on while we
    // were switching. Bounded so a tab that keeps changing cannot spin.
    for _ in 0..2 {
        let began = Instant::now();
        let tab = slingr::herdr::focused();
        // One query where there were three. The focused window already carries the
        // workspace it is in and the screen it is on, and asking AeroSpace
        // separately for each is three process spawns on the path of every tab
        // switch. An empty workspace has no focused window, so that case still has
        // to ask — which is the reason the direct query exists at all.
        let focused = wm.focused_window();
        let here = match &focused {
            Some(w) => w.workspace.clone(),
            None => wm.focused_workspace().unwrap_or_default(),
        };

        // No window counts: an empty task is somewhere to go, so nothing here
        // needs to know how full anything is. Listing every window on the hot
        // path is what made this feel slow.
        let counts = std::collections::BTreeMap::new();

        // No settling to wait for: the hook fires once per actual focus change,
        // so the tab has already stopped moving by the time we are called.
        match decide(tab.as_deref(), tab.as_deref(), &here, &counts) {
            Action::Switch(target) => {
                let follow = FollowList::load();

                // Whatever you are typing in — normally the terminal herdr itself
                // runs in, since changing a tab is what got us here.
                //
                // Only a window that follows you counts. Preserving whatever
                // happens to hold focus sounds more general and is worse: let
                // anything else grab focus once — an app raising itself, a
                // notification — and every switch afterwards faithfully hands
                // focus back to it. A follower is with you by design; anything
                // else is a coincidence.
                let hands_on = focused.clone().filter(|w| {
                    follow.ids().contains(&w.id)
                        || follow.follows_app(&w.bundle)
                        || slingr::picker::is_global_by_nature(&w.bundle)
                });

                // Bring the followers before switching, not after. AeroSpace runs
                // `follow` on the change as well, but doing it here means one
                // visible rearrangement instead of two.
                let on = match &focused {
                    Some(w) => Some(w.monitor.clone()),
                    None => wm.focused_monitor(),
                };
                let brought = app::follow_to(
                    wm.as_ref(),
                    app::Following {
                    windows: &follow.ids(),
                    apps: &follow.bundles(),
                    grounded: &follow.grounded_ids(),
                },
                    &target,
                    "",
                    None,
                    on.as_deref(),
                );

                // Arrive by focusing the window you are typing in, rather than by
                // asking for the workspace and then correcting the focus it chose.
                // Focusing a window goes to its workspace, so this is one
                // operation with no window in between — and nothing to race with
                // the callback AeroSpace fires on the change.
                //
                // Asking for the workspace focuses whatever the destination
                // happened to hold. That is right when you asked for a workspace,
                // and wrong when you asked for a herdr tab: it takes the keyboard
                // out from under you, and the next tab click is spent getting it
                // back.
                // Read off what was just moved rather than asking again. The
                // window came along if it was carried, or was already there.
                let came_along = hands_on.as_ref().is_some_and(|w| {
                    w.workspace == target
                        || brought.iter().any(|(moved, outcome)| {
                            moved.id == w.id && matches!(outcome, Outcome::Moved { .. })
                        })
                });
                // Said before the switch, so the callback it triggers can see it.
                store::Echo::say(&target);
                let hands_kept = matches!((came_along, &hands_on), (true, Some(_)));
                match (came_along, hands_on) {
                    (true, Some(w)) => {
                        wm.focus(&w.id);
                    }
                    _ => {
                        wm.focus_workspace(&target);
                    }
                }
                say!("{here} -> {target}");
                note(&json!({
                    "at": store::iso8601(store::now_unix()),
                    "goto": target, "from": here,
                    "kept_focus": hands_kept,
                    // How many windows were rearranged before the switch became
                    // visible. This is what a "flash" is made of.
                    "brought": brought.len(),
                    "ms": began.elapsed().as_millis(),
                }));
                // A tab clicked while this switch was in flight was turned
                // away by the lock above, so without looking again the
                // earlier — and by now staler — intent wins: two quick taps
                // land you on the first. Re-reading herdr is a socket
                // round-trip, not a process.
                if !slingr::watch::look_again(&target, slingr::herdr::focused().as_deref()) {
                    break;
                }
            }
            Action::Hold(why) => {
                if let Some(t) = &tab {
                    say!("holding: {why} ({t})");
                }
                note(&json!({
                    "at": store::iso8601(store::now_unix()),
                    "hold": why, "tab": tab, "from": here,
                }));
                break;
            }
        }
    }
    Ok(())
}

/// A line per herdr-driven switch, so "it sometimes does nothing" is a thing
/// that can be read rather than guessed at.
fn note(entry: &serde_json::Value) {
    let _ = ActionLog::watch().append(entry);
}

fn notify_from_hook() -> Result<()> {
    use slingr::attention::{self, Report};

    let Ok(json) = std::env::var("HERDR_PLUGIN_EVENT_JSON") else {
        say!("nothing to do: run by herdr's pane.agent_status_changed hook, which says what changed");
        return Ok(());
    };
    let Some(change) = attention::parse_change(&json) else {
        // herdr's plugin log keeps what a hook prints, so a payload that has
        // changed shape can be read there rather than guessed at.
        eprintln!("not an agent status change: {json}");
        return Ok(());
    };
    let report = attention::on_change(&attention::Live::from_env(), &change);
    say!("{} {}: {report:?}", change.pane, change.status);
    // An agent starting or stopping work is nearly every change, and says
    // nothing worth keeping.
    if report == Report::Quiet {
        return Ok(());
    }
    let mut entry = json!({
        "at": store::iso8601(store::now_unix()),
        "pane": change.pane,
        "status": change.status,
        // herdr's own words, kept whole: its hooks and its socket spell this
        // event differently, and the next change of spelling should be
        // readable here (docs/FINDINGS.md).
        "event": serde_json::from_str::<serde_json::Value>(&json).unwrap_or_default(),
    });
    match report {
        Report::Quiet => {}
        Report::Posted { kind, title } => {
            entry["attention"] = json!("posted");
            entry["kind"] = json!(kind);
            entry["title"] = json!(title);
        }
        Report::NotPosted => entry["attention"] = json!("not posted"),
        Report::Withdrawn => entry["attention"] = json!("withdrawn"),
        Report::Left(why) => {
            entry["attention"] = json!("unsaid");
            entry["why"] = json!(why);
        }
        Report::Busy => entry["attention"] = json!("busy"),
    }
    note(&entry);
    Ok(())
}

fn answer(pane: &str, tab: Option<&str>) -> Result<()> {
    let went = slingr::attention::answer(&slingr::attention::Live::from_env(), pane, tab);
    note(&json!({
        "at": store::iso8601(store::now_unix()),
        "answer": pane,
        "tab": tab,
        "went": format!("{went:?}").to_lowercase(),
    }));
    Ok(())
}

fn follow_now() -> Result<()> {
    // `goto` has left a line per switch since the beginning; this one left
    // none, so "it feels like things happen more than once" could only be
    // argued about. One line per run says how many times AeroSpace actually
    // fired the hook, and what each run cost.
    // Two of these running at once move the same windows twice, in a race,
    // each undoing part of what the other just did. Measured: two concurrent
    // runs moving seven windows apiece, which is most of what "flashing" is.
    // `goto` has had this lock since the beginning; this had none.
    let held = only_one("follow");
    let mut catch = slingr::watch::catch(held.is_none(), store::Settling::Settled);
    if catch == slingr::watch::Catch::Busy {
        note(&json!({ "at": store::iso8601(store::now_unix()), "skipped": "follow already running" }));
        return Ok(());
    }
    let began = Instant::now();
    let wm = wm::open(Patience::Query);
    let follow = FollowList::load();
    let Some(here) = wm.focused_workspace() else {
        say!("{} did not answer", wm.name());
        return Ok(());
    };
    // When our own `goto` caused this change, the followers were brought a
    // moment ago — by the same process, before the switch was even visible.
    // Asking AeroSpace for all forty-odd windows again, to discover there is
    // nothing left to do, was half the work of a tab switch done twice.
    //
    // And when a switch of ours is still in flight, the workspace AeroSpace
    // reports may be the one being *left*. Neither moving followers there nor
    // telling herdr about it is right; the settled state will arrive by
    // itself.
    //
    // A follower that refused to move is not retried here; it is picked up by
    // the next change that was not ours.
    catch = slingr::watch::catch(false, store::Echo::settling(&here));
    if catch == slingr::watch::Catch::MidSwitch {
        note(&json!({
            "at": store::iso8601(store::now_unix()),
            "skipped": "mid-switch", "reported": here,
        }));
        return Ok(());
    }
    let moved = if !catch.brings_followers() {
        0
    } else {
        // Only the screen being worked on. Clicking a window on another
        // display must not pull everything across to it.
        let on = wm.focused_monitor();
        let brought = app::follow_to(
            wm.as_ref(),
            app::Following {
                windows: &follow.ids(),
                apps: &follow.bundles(),
                grounded: &follow.grounded_ids(),
            },
            &here,
            "",
            None,
            on.as_deref(),
        );
        if !brought.is_empty() {
            say!("brought {} to {here}", brought.len());
        }
        brought.len()
    };

    // The other half of the pairing. A task is a herdr tab and a workspace, so
    // arriving at the workspace should bring the tab with it — whether you got
    // here by keybinding, by the menu bar, or by clicking a window.
    //
    // This cannot chase its own tail: focusing a tab that is already focused
    // is skipped, and were it not, the return trip finds the workspace already
    // correct and stops there.
    // Unless we are the reason it changed. Reporting our own switch back to
    // herdr is what makes the pair chase each other.
    if catch.moves_the_tab() {
        if let Some(tab) = slingr::herdr::tab_named(&here) {
            if !tab.focused && slingr::herdr::focus_tab(&tab.id) {
                say!("herdr -> {here}");
            }
        }
    }

    // A snapshot is for putting things back after a restart, so it only has to
    // be roughly current. Taken on every workspace change it costs a full
    // window listing and an 8KB write on the hot path — about a fifth of a tab
    // switch — to write down a layout that has usually not moved since the
    // last one. `restore` prefers the action log anyway, which records intent
    // and is written the moment anything is slung.
    //
    // Not at all where windows cannot be moved: there is no restore to feed,
    // and macOS keeps each window on its Space without being asked.
    let snapped = if wm.moves_windows() && snapshot_is_stale() { snapshot().unwrap_or(0) } else { 0 };
    note(&json!({
        "at": store::iso8601(store::now_unix()),
        "follow": here,
        "snapped": snapped,
        "brought": moved,
        "catch": format!("{catch:?}"),
        "ms": began.elapsed().as_millis(),
    }));
    Ok(())
}

fn list_following(prune: bool) -> Result<()> {
    let mut follow = FollowList::load();

    // These follow without anyone having asked, so they belong at the top of
    // the answer to "what follows me" even though they are in no file.
    for bundle in slingr::picker::GLOBAL_BY_NATURE {
        say!("  always {bundle}  (global by nature — sling one to leave it behind)");
    }
    if follow.windows.is_empty() && follow.apps.is_empty() && follow.grounded.is_empty() {
        say!("  nothing else follows you yet — pick \"all workspaces\" in the picker");
        return Ok(());
    }

    for a in &follow.apps {
        say!("  every  {} ({})", a.name, a.bundle);
    }

    // Window ids do not survive the application restarting, so an entry can
    // outlive the window it named. Such an entry is skipped anyway, but a list
    // that does not say so is worse than no list.
    let live: Vec<String> = wm::open(Patience::Query)
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

    let mut grounded_stale = 0;
    for w in &follow.grounded {
        let gone = !live.is_empty() && !live.contains(&w.id);
        if gone {
            grounded_stale += 1;
        }
        say!(
            "  {} [{}] {} — {}  (slung, so it stays put)",
            if gone { "gone" } else { "    " },
            w.id,
            w.app,
            w.title
        );
    }
    stale += grounded_stale;

    if stale > 0 && prune {
        follow.windows.retain(|w| live.contains(&w.id));
        follow.grounded.retain(|w| live.contains(&w.id));
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
    if run.occupancy.is_none() {
        // `aerospace_unavailable` for AeroSpace, as it always was, so the log
        // reads the same across the change; `spaces_unavailable` for Spaces.
        let backend = format!("{:?}", wm::configured()).to_lowercase();
        entry[format!("{backend}_unavailable")] = json!(true);
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
    match &batch.target {
        Some(target) => say!("moved {moved} of {size} to {target}"),
        // The board sends windows to several tasks at once, so there is no one
        // destination to name.
        None => say!("moved {moved} of {size}"),
    }
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

fn watch(dry_run: bool, poll: bool, interval_ms: u64, settle_ms: u64) -> Result<()> {
    let wm = wm::open(Patience::Switch);
    let settle = Duration::from_millis(settle_ms);

    if poll {
        return watch_by_polling(wm.as_ref(), dry_run, Duration::from_millis(interval_ms.max(100)));
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

    // The window manager's own events, so the follow list keeps up with a
    // workspace change from any cause — a keybinding, a click, or sling
    // itself. Moving a window by id changes neither focus nor workspace, so
    // this cannot feed itself: bringing followers raises no further event.
    if !dry_run {
        thread::spawn(move || {
            let source = wm::open(Patience::Query);
            let Some(changes) = source.workspace_changes() else { return };
            let mut last = String::new();
            for workspace in changes {
                if workspace == last {
                    continue;
                }
                last = workspace.clone();
                let follow = FollowList::load();
                if !follow.windows.is_empty() {
                    let on = source.focused_monitor();
                    app::follow_to(
                        source.as_ref(),
                        app::Following {
                windows: &follow.ids(),
                apps: &follow.bundles(),
                grounded: &follow.grounded_ids(),
            },
                        &workspace,
                        "",
                        None,
                        on.as_deref(),
                    );
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
        act_on(wm.as_ref(), slingr::herdr::focused(), before, dry_run);
    }
    say!("herdr closed the connection");
    Ok(())
}

fn watch_by_polling(wm: &dyn WindowManager, dry_run: bool, wait: Duration) -> Result<()> {
    say!(
        "polling herdr every {}ms{}  (ctrl-c to stop)",
        wait.as_millis(),
        if dry_run { ", dry run" } else { "" }
    );
    let mut settled: Option<String> = None;
    loop {
        let tab = slingr::herdr::focused();
        act_on(wm, tab.clone(), settled, dry_run);
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
fn act_on(wm: &dyn WindowManager, tab: Option<String>, settled: Option<String>, dry_run: bool) {
    use slingr::watch::{decide, Action};

    let began = Instant::now();
    let focused = wm.focused_window();
    let here = wm.focused_workspace().unwrap_or_default();
    let occupancy = wm.occupancy().unwrap_or_default();
    // The log and `decide` both only ever wanted the numbers; the icon stack
    // is a panel concern and has no business in the watch record.
    let counts: std::collections::BTreeMap<String, usize> =
        occupancy.iter().map(|(w, o)| (w.clone(), o.count)).collect();
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
                let on = wm.focused_monitor();
                let brought = app::follow_to(
                    wm,
                    app::Following {
                windows: &follow.ids(),
                apps: &follow.bundles(),
                grounded: &follow.grounded_ids(),
            },
                    &target,
                    "",
                    Some(&here),
                    on.as_deref(),
                );
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
                let ok = wm.focus_workspace(&target);
                let switch_ms = t1.elapsed().as_millis();

                // Did it stay? A window left behind in the old workspace can
                // pull focus straight back, which reads as a flash and no
                // apparent change.
                thread::sleep(Duration::from_millis(1200));
                let after = wm.focused_window();

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

/// Teach the window manager every task we know about, from every source at
/// once.
///
/// This is what lets a brand-new task be switched to, appear in AeroSpace's
/// menu bar, and be offered by name — without sling keeping a list of its own.
fn sync_workspaces(loud: bool) -> Result<()> {
    let cfg = Config::load()?;
    let wm = wm::open_for(cfg.backend, Patience::Query);

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
    add(wm.all_workspaces().unwrap_or_default());
    // Whatever the old remembered-names file still holds, so nothing that was
    // invented before this existed is lost.
    add(store::NameCache::load().workspaces);
    names.sort();

    let changed = match wm.sync_workspaces(&names) {
        Synced::Written => true,
        Synced::Unchanged => false,
        Synced::Refused(why) => {
            say!("{why}");
            return Ok(());
        }
    };
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
/// How old the layout may get before a workspace change bothers to retake it.
const SNAPSHOT_EVERY: Duration = Duration::from_secs(90);

fn snapshot_is_stale() -> bool {
    let Ok(meta) = std::fs::metadata(store::state_dir().join("layout.json")) else {
        return true;
    };
    meta.modified()
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_none_or(|age| age >= SNAPSHOT_EVERY)
}

fn snapshot() -> Result<usize> {
    let Some(windows) = wm::open(Patience::Query).all_windows() else {
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
    let wm = wm::open(Patience::Query);
    if !wm.moves_windows() {
        say!("{} cannot move windows, and keeps each one on its Space by itself", wm.name());
        return Ok(());
    }
    let Some(live) = wm.all_windows() else {
        say!("{} did not answer", wm.name());
        return Ok(());
    };

    // Windows that belong everywhere are wherever you are on purpose. Putting
    // them back where they once were would undo the thing they are for.
    let follow = FollowList::load();
    let followed =
        |w: &slingr::picker::Window| follow.ids().contains(&w.id) || follow.follows_app(&w.bundle);

    let layout = Layout::load();
    let by_id: std::collections::BTreeMap<&str, &str> = layout
        .windows
        .iter()
        .map(|p| (p.id.as_str(), p.workspace.as_str()))
        .collect();
    let by_name: std::collections::BTreeMap<(&str, &str), &str> = layout
        .windows
        .iter()
        .map(|p| ((p.app.as_str(), p.title.as_str()), p.workspace.as_str()))
        .collect();
    let logged = store::placements_from_log();

    let began = Instant::now();
    let (mut moved, mut failed, mut already, mut skipped, mut unknown) = (0, 0, 0, 0, 0);
    let (mut from_snapshot, mut from_log) = (0, 0);

    for w in &live {
        if followed(w) {
            skipped += 1;
            continue;
        }
        // The log is asked first, because it records intent and the snapshot
        // records happenstance. A window deliberately slung to a task should
        // go back there even though a later snapshot merely observed it
        // sitting somewhere else — which is exactly what a reboot produces,
        // since the automatic snapshot writes down the scattered state.
        //
        // The snapshot then covers everything that was never slung by hand.
        let want = logged
            .iter()
            .find(|((app, title), _)| *app == w.app && *title == w.title)
            .map(|(_, ws)| (ws.as_str(), false))
            .or_else(|| by_id.get(w.id.as_str()).map(|ws| (*ws, true)))
            .or_else(|| by_name.get(&(w.app.as_str(), w.title.as_str())).map(|ws| (*ws, true)));

        let Some((target, exact)) = want else {
            unknown += 1;
            continue;
        };
        if w.workspace == target {
            already += 1;
            continue;
        }
        if dry_run {
            say!("  {} -> {target}   ({})", w.label(), if exact { "snapshot" } else { "log" });
            moved += 1;
        } else if wm.move_window(&w.id, target) {
            moved += 1;
            if exact {
                from_snapshot += 1;
            } else {
                from_log += 1;
            }
        } else {
            failed += 1;
        }
    }

    say!(
        "{moved} moved, {already} already right, {skipped} follow you, {unknown} never placed, {failed} failed ({:.1}s)",
        began.elapsed().as_secs_f32()
    );
    if from_snapshot > 0 || from_log > 0 {
        say!("  {from_snapshot} from the snapshot, {from_log} from the action log");
    }
    Ok(())
}

/// Read-only: the two queries a sling starts with, and nothing else.
fn probe() -> Result<()> {
    let wm = wm::open(Patience::Query);
    say!("backend  {}", wm.name());
    say!("focused  {}", wm.probe_focused());
    match wm.occupancy() {
        Some(counts) => {
            // The stack too, because "what will the panel draw on this row"
            // is exactly the question probe exists to answer.
            for (workspace, o) in &counts {
                let apps: Vec<&str> = o
                    .stack
                    .iter()
                    .map(|b| b.rsplit('.').next().unwrap_or(b.as_str()))
                    .collect();
                say!("occupied {workspace} ({}) {}", o.count, apps.join(" "));
            }
        }
        None => say!("occupied FAILED"),
    }
    let cache = NameCache::load();
    say!("herdr    {}", slingr::herdr::tasks().join(", "));
    say!("known    {}", Config::load()?.known.join(", "));
    say!("seen     {}", cache.workspaces.join(", "));
    Ok(())
}

/// Every screen's Spaces, in Mission Control's order, with the task each is
/// for — the picture Mission Control would draw if a Space could have a name.
fn list_spaces() -> Result<()> {
    use slingr::spaces::{names, Bindings, Spaces};

    let spaces = Spaces::default();
    let Some(state) = spaces.state() else {
        say!("{} did not answer", spaces.helper().display());
        return Ok(());
    };
    let names = names(&state, &Bindings::load());
    for display in &state.displays {
        say!("{} {}", if display.main { "main screen" } else { "screen" }, display.id);
        for space in &display.spaces {
            let held = state.windows.iter().filter(|w| w.spaces == [space.id]).count();
            say!(
                "  {} {:<28} {held} window{}",
                if space.id == display.current { "●" } else { " " },
                names.get(&space.id).map(String::as_str).unwrap_or("?"),
                if held == 1 { "" } else { "s" }
            );
        }
    }
    let everywhere = state.windows.iter().filter(|w| w.spaces.len() > 1).count();
    if everywhere > 0 {
        say!("{everywhere} on every desktop");
    }
    if state.windows.iter().all(|w| w.title.is_empty()) {
        say!("no titles — whatever runs this needs Screen Recording");
    }
    Ok(())
}

/// For the agent. Nothing at all when the helper does not answer, so that a
/// name is never laid over a desktop it was not meant for.
fn print_labels() -> Result<()> {
    use slingr::spaces::{labels, Bindings, Spaces};

    if let Some(state) = Spaces::default().state() {
        say!("{}", serde_json::to_string(&labels(&state, &Bindings::load()))?);
    }
    Ok(())
}

fn bind(task: &str) -> Result<()> {
    let name = slingr::picker::sanitise_workspace(task);
    if name.is_empty() {
        say!("{task:?} is not a name a task can have");
        return Ok(());
    }
    let was = slingr::spaces::Spaces::default().bind_here(&name)?;
    say!("{was} is now {name}");
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A click runs a command written into a string, which no compiler reads:
    /// a renamed subcommand would leave every notification going nowhere.
    #[test]
    fn a_click_runs_a_command_slingr_has() {
        let parsed = Cli::try_parse_from([
            "slingr",
            slingr::attention::ANSWER,
            "w7:p2J",
            "--tab",
            "w7:t2J",
        ])
        .expect("answer is a subcommand");
        assert!(matches!(
            parsed.command,
            Some(Cmd::Answer { pane, tab: Some(tab) }) if pane == "w7:p2J" && tab == "w7:t2J"
        ));
    }

    /// herdr runs what the manifest names, and a name that is not a
    /// subcommand fails only inside herdr's plugin log.
    #[test]
    fn the_plugin_runs_only_commands_slingr_has() {
        let manifest: toml::Value =
            toml::from_str(include_str!("../plugin/herdr-plugin.toml.example")).expect("toml");
        let events = manifest["events"].as_array().expect("events");
        for event in events {
            let command = event["command"].as_array().expect("command");
            let args: Vec<&str> = command.iter().map(|a| a.as_str().unwrap()).collect();
            assert_eq!(args[0], "SLINGR_BINARY", "install.sh fills in the path");
            assert!(Cli::try_parse_from(&args).is_ok(), "{args:?} is not a slingr command");
        }
        let notify = events
            .iter()
            .find(|e| e["on"].as_str() == Some("pane.agent_status_changed"))
            .expect("the hook that makes notifications clickable");
        assert_eq!(notify["command"][1].as_str(), Some("notify"));
    }
}
