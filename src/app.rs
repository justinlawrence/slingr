//! The sling itself: read the focused window, ask where it belongs, move it.
//!
//! Written against traits rather than AeroSpace so the whole flow can be
//! exercised without a window manager — which is how the bug that moved the
//! wrong window is now pinned down in `tests/flow.rs`.

use std::collections::BTreeMap;

use crate::aerospace::WindowManager;
use crate::config::Config;
use crate::dialog::Prompt;
use crate::picker::{self, Window, ALL, NEW, TO_JUMP, TO_MANY, TO_ONE};

/// The application that owns sling's own dialog; never a task window.
const DIALOG_APP: &str = "System Events";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing focused, or AeroSpace did not answer.
    NoWindow,
    /// The focused window is sling's own dialog. Pressing the key twice in a
    /// row lands here, because System Events keeps focus after the dialog
    /// closes.
    OwnDialog,
    Cancelled,
    /// A group heading was selected; headings name no workspace.
    Heading,
    /// A tab was picked. Never reaches the log — `run_session` acts on it and
    /// reopens in that mode. Carries which, because with three tabs there is
    /// no "the other one".
    SwitchMode { to: String },
    /// A row was pinned or unpinned. The caller stores it and asks again,
    /// since the order of the list has changed underneath.
    PinToggled { workspace: String },
    SameWorkspace,
    /// A typed name that sanitised away to nothing.
    EmptyName { raw: String },
    /// The window could not be focused again after the dialog, so nothing was
    /// moved. Minimised windows land here.
    RefocusFailed,
    MoveFailed { to: String },
    Moved { to: String, created: bool },
    /// Added to the follow list: comes along to every task from now on.
    Following,
    /// Taken off it.
    Unfollowing,
    /// Went to a task rather than sending anything to one.
    Jumped { to: String },
}

impl Outcome {
    pub fn kind(&self) -> &'static str {
        match self {
            Outcome::NoWindow => "no_window",
            Outcome::OwnDialog => "own_dialog",
            Outcome::Cancelled => "cancelled",
            Outcome::Heading => "heading",
            Outcome::SwitchMode { .. } => "switch_mode",
            Outcome::PinToggled { .. } => "pin_toggled",
            Outcome::SameWorkspace => "same_workspace",
            Outcome::EmptyName { .. } => "empty_name",
            Outcome::RefocusFailed => "refocus_failed",
            Outcome::MoveFailed { .. } => "move_failed",
            Outcome::Moved { .. } => "moved",
            Outcome::Following => "following",
            Outcome::Unfollowing => "unfollowing",
            Outcome::Jumped { .. } => "jumped",
        }
    }
}

pub struct Run {
    pub window: Option<Window>,
    /// Windows per workspace, or `None` if AeroSpace did not answer.
    pub counts: Option<BTreeMap<String, usize>>,
    pub outcome: Outcome,
    /// Followers dragged along to the same task.
    pub brought: Vec<(Window, Outcome)>,
}

/// Move the follow list to `target`, skipping the window that was slung there
/// in its own right and any follower already sitting in it.
///
/// `only_from` is a safety rail, not a filter for tidiness. Moving a window
/// requires focusing it, and focusing a window in a workspace that is off
/// screen makes AeroSpace restore that entire workspace first — about three
/// seconds per window it holds. Fetching a follower out of a crowded pool
/// would stall for a minute. Passing the workspace that is on screen keeps
/// every follower move cheap; a follower that has wandered elsewhere is left
/// alone until it is slung back by hand.
///
/// The window list is only fetched when there is something to follow, so the
/// ordinary no-followers case costs nothing extra.
pub fn follow_to(
    wm: &dyn WindowManager,
    follow: &[String],
    target: &str,
    already_moved: &str,
    only_from: Option<&str>,
    only_on: Option<&str>,
) -> Vec<(Window, Outcome)> {
    if follow.is_empty() {
        return Vec::new();
    }
    let Some(all) = wm.all_windows() else {
        return Vec::new();
    };
    let mut brought = Vec::new();
    for w in all.into_iter().filter(|w| {
        follow.contains(&w.id)
            && w.id != already_moved
            && w.workspace != target
            && only_from.is_none_or(|from| w.workspace == from)
            && only_on.is_none_or(|monitor| w.monitor == monitor)
    }) {
        let outcome = if wm.move_window(&w.id, target) {
            Outcome::Moved { to: target.to_string(), created: false }
        } else {
            Outcome::MoveFailed { to: target.to_string() }
        };
        brought.push((w, outcome));
    }
    brought
}

/// What moving the follow list will cost, in workspace restores.
///
/// Moving a window makes AeroSpace's view follow it, so after the first move
/// the remaining followers are in a workspace that is no longer on screen.
/// Fetching each of those restores its workspace and then the target again:
/// `1 + 2(n-1)` restores for `n` followers, at roughly three seconds per
/// window in each workspace restored. One follower is cheap. Two is not.
pub fn restores_for(followers: usize) -> usize {
    // Since moving by id neither focuses nor switches, following costs no
    // workspace restores at all. Kept so the old arithmetic stays documented.
    let _ = followers;
    0
}

pub fn run(
    wm: &dyn WindowManager,
    prompt: &dyn Prompt,
    cfg: &Config,
    cached: &[String],
    follow: &[String],
    pins: &[String],
) -> Run {
    let Some(window) = wm.focused_window() else {
        return Run { window: None, counts: None, outcome: Outcome::NoWindow, brought: Vec::new() };
    };
    // System Events owns the dialog sling draws. Slinging it would move a
    // dialog into a task and leave the real window where it was.
    if window.app == DIALOG_APP {
        return Run {
            window: Some(window),
            counts: None,
            outcome: Outcome::OwnDialog,
            brought: Vec::new(),
        };
    }

    let counts = wm.window_counts();
    // Everything AeroSpace knows about, which with persistent-workspaces is
    // every task — including the ones holding nothing yet.
    let known_to_aerospace = wm.all_workspaces().unwrap_or_default();
    let menu = picker::build_menu(
        counts.as_ref(),
        &cfg.known,
        &[cached, &known_to_aerospace].concat(),
        &window.workspace,
        &cfg.order.prefixes,
        &cfg.labels,
        pins,
    );

    let mut rows = tabs(Mode::One);
    rows.push(picker::Row::subject(&window.app, &window.title, &window.bundle));
    rows.extend(menu.rows.clone());
    // Sits with the current workspace, because both answer the same question:
    // where this window lives. An ordinary row, so it can carry a tick.
    let showing_everywhere = follow.contains(&window.id);
    if showing_everywhere {
        // A window shown everywhere is not in any one workspace, so the
        // workspace it happens to be sitting in must not claim it too.
        for row in &mut rows {
            if row.marker.as_deref() == Some("here") {
                row.marker = None;
            }
        }
    }
    // Whichever of the two is true goes first: the group should open with the
    // answer, not with the option that was not taken.
    let at = if showing_everywhere {
        rows.iter().position(|r| r.section.as_deref() == Some("here")).unwrap_or(rows.len())
    } else {
        rows.iter()
            .rposition(|r| r.section.as_deref() == Some("here"))
            .map(|found| found + 1)
            .unwrap_or(rows.len())
    };
    rows.insert(at, picker::Row::toggle(ALL, "Show on all workspaces", showing_everywhere));

    // The window itself is shown in the header, icon and all, so this only
    // has to carry anything unusual.
    let mut heading = String::new();
    if counts.is_none() {
        heading.push_str("AeroSpace is not answering — these are remembered names");
    }

    let done = |outcome| Run {
        window: Some(window.clone()),
        counts: counts.clone(),
        outcome,
        brought: Vec::new(),
    };

    let Some(choice) = prompt.choose_rows(&rows, "Sling window", &heading) else {
        return done(Outcome::Cancelled);
    };
    if choice == TO_MANY || choice == TO_JUMP {
        return done(Outcome::SwitchMode { to: choice });
    }
    if choice == TO_ONE {
        // Already here; nothing to switch to.
        return done(Outcome::Cancelled);
    }
    if let Some(workspace) = choice.strip_prefix(picker::PIN) {
        return done(Outcome::PinToggled { workspace: workspace.to_string() });
    }
    if choice == ALL {
        // A standing instruction, not a destination: the window stays where it
        // is and comes along next time something is slung.
        return done(if follow.contains(&window.id) {
            Outcome::Unfollowing
        } else {
            Outcome::Following
        });
    }

    let target = if choice == NEW {
        let Some(raw) = prompt.ask_text("Sling to a new workspace", "Name the new workspace") else {
            return done(Outcome::Cancelled);
        };
        let name = picker::sanitise_workspace(&raw);
        if name.is_empty() {
            return done(Outcome::EmptyName { raw });
        }
        name
    } else if rows.iter().any(|r| r.id == choice && r.marker.as_deref() != Some("action")) {
        choice.clone()
    } else {
        return done(Outcome::Heading);
    };

    if target == window.workspace {
        return done(Outcome::SameWorkspace);
    }

    // Named, not focused. The dialog stealing focus stops mattering, and a
    // window that cannot take focus — minimised, say — moves like any other.
    if !wm.move_window(&window.id, &target) {
        return done(Outcome::MoveFailed { to: target });
    }

    let created = counts.as_ref().map(|c| !c.contains_key(&target)).unwrap_or(false);
    // Anywhere on this screen: moving by id costs nothing, but a follower on
    // another display is beside work of its own and should stay there.
    let brought = follow_to(wm, follow, &target, &window.id, None, Some(&window.monitor));
    Run {
        window: Some(window.clone()),
        counts: counts.clone(),
        outcome: Outcome::Moved { to: target, created },
        brought,
    }
}

/// The result of moving several windows at once.
pub struct Batch {
    pub target: Option<String>,
    pub results: Vec<(Window, Outcome)>,
    /// Set when the batch never reached the point of moving anything.
    pub aborted: Option<Outcome>,
}

impl Batch {
    fn stopped(outcome: Outcome) -> Self {
        Self { target: None, results: Vec::new(), aborted: Some(outcome) }
    }

    pub fn moved(&self) -> usize {
        self.results.iter().filter(|(_, o)| matches!(o, Outcome::Moved { .. })).count()
    }
}

/// Select several windows, then send them all to one task.
///
/// Built for draining: a workspace that has accumulated dozens of windows is
/// emptied by selecting a handful at a time, and moving a window *out* of the
/// visible workspace is cheap — unlike switching into a crowded one.
pub fn run_many(
    wm: &dyn WindowManager,
    prompt: &dyn Prompt,
    cfg: &Config,
    cached: &[String],
    follow: &[String],
    pins: &[String],
) -> Batch {
    let Some(all) = wm.all_windows() else {
        return Batch::stopped(Outcome::NoWindow);
    };
    let windows: Vec<Window> = all.into_iter().filter(|w| w.app != DIALOG_APP).collect();
    if windows.is_empty() {
        return Batch::stopped(Outcome::NoWindow);
    }

    let here = wm.focused_workspace().unwrap_or_default();
    let counts = wm.window_counts().unwrap_or_default();

    // Which windows, then where. The list is grouped by the workspace each
    // window is in, and a whole group can be taken at once — so the folders
    // are here, in the answer, rather than as a question of their own.
    let from = windows.clone();
    let menu = picker::build_window_menu(&from, &here, follow);
    let mut rows = tabs(Mode::Many);
    rows.extend(menu.rows.clone());

    let Some(picked) = prompt.choose_many_rows(
        &rows,
        "Sling several windows",
        "which windows belong together?",
    ) else {
        return Batch::stopped(Outcome::Cancelled);
    };
    if let Some(tab) = picked.iter().find(|p| *p == TO_ONE || *p == TO_JUMP) {
        return Batch::stopped(Outcome::SwitchMode { to: tab.clone() });
    }
    if let Some(tab) = picked.iter().find(|p| *p == TO_ONE || *p == TO_JUMP) {
        return Batch::stopped(Outcome::SwitchMode { to: tab.clone() });
    }
    let chosen: Vec<Window> =
        from.iter().filter(|w| picked.contains(&w.id)).cloned().collect();
    if chosen.is_empty() {
        return Batch::stopped(Outcome::Cancelled);
    }

    let known_to_aerospace = wm.all_workspaces().unwrap_or_default();
    let targets = picker::build_menu(
        Some(&counts),
        &cfg.known,
        &[cached, &known_to_aerospace].concat(),
        "",
        &cfg.order.prefixes,
        &cfg.labels,
        pins,
    );
    let heading = format!(
        "{} window{} looking for a home",
        chosen.len(),
        if chosen.len() == 1 { "" } else { "s" }
    );
    let mut target_rows = targets.rows.clone();
    target_rows.insert(1, picker::Row::action(ALL, &picker::all_row(false)));
    let Some(choice) = prompt.choose_rows(&target_rows, "Sling several windows", &heading) else {
        return Batch::stopped(Outcome::Cancelled);
    };
    if choice == ALL {
        // Tag the selection instead of moving it.
        return Batch {
            target: None,
            results: chosen.into_iter().map(|w| (w, Outcome::Following)).collect(),
            aborted: None,
        };
    }

    let target = if choice == NEW {
        let Some(raw) = prompt.ask_text("Sling to a new workspace", "Name the new workspace") else {
            return Batch::stopped(Outcome::Cancelled);
        };
        let name = picker::sanitise_workspace(&raw);
        if name.is_empty() {
            return Batch::stopped(Outcome::EmptyName { raw });
        }
        name
    } else if target_rows.iter().any(|r| r.id == choice && r.marker.as_deref() != Some("action")) {
        choice.clone()
    } else {
        return Batch::stopped(Outcome::Heading);
    };

    // Noted before the list is consumed: the followers belong to the screen
    // the batch came from.
    let on = chosen.first().map(|w| w.monitor.clone());

    let mut results = Vec::new();
    for window in chosen {
        let outcome = if window.workspace == target {
            Outcome::SameWorkspace
        } else if !wm.move_window(&window.id, &target) {
            Outcome::MoveFailed { to: target.clone() }
        } else {
            let created = !counts.contains_key(&target);
            Outcome::Moved { to: target.clone(), created }
        };
        results.push((window, outcome));
    }

    let brought = follow_to(wm, follow, &target, "", None, on.as_deref());
    results.extend(brought);

    Batch { target: Some(target), results, aborted: None }
}

/// Which list the dialog is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    One,
    Many,
    Jump,
}

/// The three tabs, with the showing one marked. Built in one place so every
/// mode offers the same strip and cannot drift.
pub fn tabs(showing: Mode) -> Vec<picker::Row> {
    vec![
        picker::Row::tab(TO_ONE, picker::ONCE_LABEL, showing == Mode::One),
        picker::Row::tab(TO_MANY, picker::MANY_LABEL, showing == Mode::Many),
        picker::Row::tab(TO_JUMP, picker::JUMP_LABEL, showing == Mode::Jump),
    ]
}

/// Go to a task rather than sending a window to one.
///
/// Nothing is slung, so there is no subject and no window to speak of. What
/// follows from arriving — the windows that belong everywhere catching up, and
/// herdr focusing the matching tab — happens by itself, because those hang off
/// the workspace changing rather than off sling.
pub fn run_jump(
    wm: &dyn WindowManager,
    prompt: &dyn Prompt,
    cfg: &Config,
    cached: &[String],
    pins: &[String],
) -> Run {
    let counts = wm.window_counts();
    let here = wm.focused_workspace().unwrap_or_default();
    let known_to_aerospace = wm.all_workspaces().unwrap_or_default();

    let menu = picker::build_menu(
        counts.as_ref(),
        &cfg.known,
        &[cached, &known_to_aerospace].concat(),
        &here,
        &cfg.order.prefixes,
        &cfg.labels,
        pins,
    );
    let mut rows = tabs(Mode::Jump);
    rows.extend(menu.rows.clone());

    let done = |outcome| Run { window: None, counts: counts.clone(), outcome, brought: Vec::new() };

    let Some(choice) = prompt.choose_rows(&rows, "Jump to a task", "") else {
        return done(Outcome::Cancelled);
    };
    if choice == TO_ONE || choice == TO_MANY {
        return done(Outcome::SwitchMode { to: choice });
    }
    if let Some(workspace) = choice.strip_prefix(picker::PIN) {
        return done(Outcome::PinToggled { workspace: workspace.to_string() });
    }
    if choice == here {
        return done(Outcome::SameWorkspace);
    }
    if !rows.iter().any(|r| r.id == choice && r.marker.is_none()) {
        return done(Outcome::Heading);
    }
    if !wm.focus_workspace(&choice) {
        return done(Outcome::MoveFailed { to: choice });
    }
    done(Outcome::Jumped { to: choice })
}

pub enum Session {
    Single(Run),
    Batch(Batch),
}

/// One keypress, either mode. The dialog carries a row that switches between
/// them, so this reopens rather than returning when that row is picked.
pub fn run_session(
    wm: &dyn WindowManager,
    prompt: &dyn Prompt,
    cfg: &Config,
    cached: &[String],
    follow: &[String],
    pins: &[String],
) -> Session {
    run_session_from(Mode::One, wm, prompt, cfg, cached, follow, pins)
}

/// The same, opened on a given tab — so a key can go straight to jumping.
pub fn run_session_from(
    start: Mode,
    wm: &dyn WindowManager,
    prompt: &dyn Prompt,
    cfg: &Config,
    cached: &[String],
    follow: &[String],
    pins: &[String],
) -> Session {
    let mut mode = start;
    // Toggling is the user's to do as often as they like; the bound is only
    // here so a prompt that always answers "switch" cannot spin forever.
    for _ in 0..16 {
        let switched = |to: &str| match to {
            TO_MANY => Mode::Many,
            TO_JUMP => Mode::Jump,
            _ => Mode::One,
        };
        match mode {
            Mode::One => {
                let run = run(wm, prompt, cfg, cached, follow, pins);
                if let Outcome::SwitchMode { to } = &run.outcome {
                    mode = switched(to);
                    continue;
                }
                return Session::Single(run);
            }
            Mode::Jump => {
                let run = run_jump(wm, prompt, cfg, cached, pins);
                if let Outcome::SwitchMode { to } = &run.outcome {
                    mode = switched(to);
                    continue;
                }
                return Session::Single(run);
            }
            Mode::Many => {
                let batch = run_many(wm, prompt, cfg, cached, follow, pins);
                if let Some(Outcome::SwitchMode { to }) = &batch.aborted {
                    mode = switched(to);
                    continue;
                }
                return Session::Batch(batch);
            }
        }
    }
    Session::Single(Run {
        window: None,
        counts: None,
        outcome: Outcome::Cancelled,
        brought: Vec::new(),
    })
}
