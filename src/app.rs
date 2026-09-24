//! The sling itself: read the focused window, ask where it belongs, move it.
//!
//! Written against traits rather than AeroSpace so the whole flow can be
//! exercised without a window manager — which is how the bug that moved the
//! wrong window is now pinned down in `tests/flow.rs`.

use std::collections::BTreeMap;

use crate::aerospace::WindowManager;
use crate::config::Config;
use crate::dialog::Prompt;
use crate::picker::{self, Window, ALL, ALL_APP, NEW, TO_JUMP, TO_MANY, TO_ONE, TO_BOARD};

/// What comes along to every task: named windows, whole applications, and the
/// applications that are everywhere by nature — minus anything you have put
/// somewhere on purpose.
#[derive(Debug, Clone, Copy, Default)]
pub struct Following<'a> {
    pub windows: &'a [String],
    pub apps: &'a [String],
    /// Windows that were slung somewhere deliberately, and so stopped being
    /// global. Only ever holds windows of an application that would otherwise
    /// follow by nature; asking for a window to be somewhere is a stronger
    /// statement than a default about its application.
    pub grounded: &'a [String],
}

impl Following<'_> {
    pub fn is_empty(&self) -> bool {
        // Never, while any application is global by nature: one of its windows
        // may be open even when nothing has been added to the list. Finder
        // always is, so this is honest rather than pessimistic.
        picker::GLOBAL_BY_NATURE.is_empty() && self.windows.is_empty() && self.apps.is_empty()
    }

    pub fn has(&self, window: &Window) -> bool {
        // A window you slung is where you asked it to be. That beats anything
        // its application is by default — but not an explicit "show this one
        // everywhere", which takes it off the grounded list instead.
        if self.grounded.iter().any(|id| *id == window.id) {
            return false;
        }
        self.windows.iter().any(|id| *id == window.id)
            || (!window.bundle.is_empty()
                && (self.apps.iter().any(|b| *b == window.bundle)
                    || picker::is_global_by_nature(&window.bundle)))
    }
}

/// What an outcome means for the follow list.
///
/// Returns whether anything changed, so the caller writes the file only when
/// it did. It lives here rather than in `main` because the interesting part is
/// a rule — slinging a window of a globally-natured application is how you say
/// "not this one" — and a rule in `main` is a rule with no tests.
pub fn absorb(follow: &mut crate::store::FollowList, outcome: &Outcome, w: &Window) -> bool {
    match outcome {
        Outcome::Following { whole_app: true } | Outcome::Unfollowing { whole_app: true } => {
            follow.toggle_app(&w.bundle, &w.app);
            true
        }
        Outcome::Following { .. } => {
            follow.toggle(&w.id, &w.app, &w.title);
            // Asking for this window everywhere outranks having put it
            // somewhere earlier.
            follow.unground(&w.id);
            true
        }
        Outcome::Unfollowing { .. } => {
            follow.toggle(&w.id, &w.app, &w.title);
            true
        }
        // The sling is the whole instruction; there is nothing else to tick.
        Outcome::Moved { .. } if picker::is_global_by_nature(&w.bundle) => {
            follow.ground(&w.id, &w.app, &w.title)
        }
        _ => false,
    }
}

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
    /// `whole_app` distinguishes following this window from following every
    /// window its application has.
    Following { whole_app: bool },
    /// Taken off it.
    Unfollowing { whole_app: bool },
    /// Went to a task rather than sending anything to one.
    Jumped { to: String },
    /// Went to one particular window from the board, which brings its task
    /// forward with it. Kept apart from `Jumped` so the log says which of the
    /// two questions was being answered.
    Focused { to: String },
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
            Outcome::Following { .. } => "following",
            Outcome::Unfollowing { .. } => "unfollowing",
            Outcome::Jumped { .. } => "jumped",
            Outcome::Focused { .. } => "focused",
        }
    }
}

pub struct Run {
    pub window: Option<Window>,
    /// What each workspace holds, or `None` if AeroSpace did not answer.
    pub occupancy: Option<BTreeMap<String, picker::Occupancy>>,
    pub outcome: Outcome,
    /// Followers dragged along to the same task.
    pub brought: Vec<(Window, Outcome)>,
}

/// Nothing is brought along by a sling.
///
/// A window that belongs everywhere belongs wherever *you* are, and slinging
/// does not move you — so sending followers to the target emptied the
/// workspace you were sitting in and left you staring at nothing. They catch
/// up by themselves the moment you actually go somewhere, because arriving is
/// what `exec-on-workspace-change` fires on.
const NOT_A_MOVE_OF_YOURS: Vec<(Window, Outcome)> = Vec::new();

/// Move the follow list to `target`, skipping the window that was slung there
/// in its own right and any follower already sitting in it.
///
/// Called when you *arrive* somewhere — the `follow` hook, `goto`, `jump` —
/// never when a window is slung away from you.
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
    follow: Following<'_>,
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
        follow.has(w)
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
    follow: Following<'_>,
    pins: &[String],
) -> Run {
    let Some(window) = wm.focused_window() else {
        return Run { window: None, occupancy: None, outcome: Outcome::NoWindow, brought: Vec::new() };
    };
    // System Events owns the dialog sling draws. Slinging it would move a
    // dialog into a task and leave the real window where it was.
    if window.app == DIALOG_APP {
        return Run {
            window: Some(window),
            occupancy: None,
            outcome: Outcome::OwnDialog,
            brought: Vec::new(),
        };
    }

    let occupancy = wm.occupancy();
    // Everything AeroSpace knows about, which with persistent-workspaces is
    // every task — including the ones holding nothing yet.
    let known_to_aerospace = wm.all_workspaces().unwrap_or_default();
    let menu = picker::build_menu(
        occupancy.as_ref(),
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
    let showing_everywhere = follow.windows.iter().any(|id| *id == window.id);
    let app_everywhere =
        !window.bundle.is_empty() && follow.apps.iter().any(|b| *b == window.bundle);
    if showing_everywhere || app_everywhere {
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
    rows.insert(at, picker::Row::toggle(ALL, "Show this window on all workspaces", showing_everywhere));
    if !window.bundle.is_empty() {
        // Finder opens and closes windows all day; following one of them is
        // useless. Following the application is the thing you mean.
        rows.insert(
            at + 1,
            picker::Row::toggle(
                ALL_APP,
                &format!("Show every {} window on all workspaces", window.app),
                app_everywhere,
            ),
        );
    }

    // The window itself is shown in the header, icon and all, so this only
    // has to carry anything unusual.
    let mut heading = String::new();
    if occupancy.is_none() {
        heading.push_str("AeroSpace is not answering — these are remembered names");
    }

    let done = |outcome| Run {
        window: Some(window.clone()),
        occupancy: occupancy.clone(),
        outcome,
        brought: Vec::new(),
    };

    let Some(choice) = prompt.choose_rows(&rows, "Sling window", &heading) else {
        return done(Outcome::Cancelled);
    };
    if choice != TO_ONE && picker::is_mode(&choice) {
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
        return done(if showing_everywhere {
            Outcome::Unfollowing { whole_app: false }
        } else {
            Outcome::Following { whole_app: false }
        });
    }
    if choice == ALL_APP {
        return done(if app_everywhere {
            Outcome::Unfollowing { whole_app: true }
        } else {
            Outcome::Following { whole_app: true }
        });
    }

    // Named in the search box: no second dialog, and the name is whatever was
    // typed, put through the same rules as any other.
    if let Some(typed) = choice.strip_prefix(picker::NEW_NAMED) {
        let name = picker::sanitise_workspace(typed);
        if name.is_empty() {
            return done(Outcome::EmptyName { raw: typed.to_string() });
        }
        if !wm.move_window(&window.id, &name) {
            return done(Outcome::MoveFailed { to: name });
        }
        return Run {
            window: Some(window.clone()),
            occupancy: occupancy.clone(),
            outcome: Outcome::Moved { to: name, created: true },
            brought: NOT_A_MOVE_OF_YOURS,
        };
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

    let created = occupancy.as_ref().map(|c| !c.contains_key(&target)).unwrap_or(false);
    Run {
        window: Some(window.clone()),
        occupancy: occupancy.clone(),
        outcome: Outcome::Moved { to: target, created },
        brought: NOT_A_MOVE_OF_YOURS,
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
    follow: Following<'_>,
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
    let occupancy = wm.occupancy().unwrap_or_default();

    // Which windows, then where. The list is grouped by the workspace each
    // window is in, and a whole group can be taken at once — so the folders
    // are here, in the answer, rather than as a question of their own.
    let from = windows.clone();
    let menu = picker::build_window_menu(&from, &here, |w| follow.has(w));
    let mut rows = tabs(Mode::Many);
    rows.extend(menu.rows.clone());

    let Some(picked) = prompt.choose_many_rows(
        &rows,
        "Sling several windows",
        "which windows belong together?",
    ) else {
        return Batch::stopped(Outcome::Cancelled);
    };
    if let Some(tab) = picked.iter().find(|p| *p != TO_MANY && picker::is_mode(p)) {
        return Batch::stopped(Outcome::SwitchMode { to: tab.clone() });
    }
    let chosen: Vec<Window> =
        from.iter().filter(|w| picked.contains(&w.id)).cloned().collect();
    if chosen.is_empty() {
        return Batch::stopped(Outcome::Cancelled);
    }

    let known_to_aerospace = wm.all_workspaces().unwrap_or_default();
    let targets = picker::build_menu(
        Some(&occupancy),
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
            results: chosen
                .into_iter()
                .map(|w| (w, Outcome::Following { whole_app: false }))
                .collect(),
            aborted: None,
        };
    }

    // Named in the search box: no second dialog, and the name is whatever was
    // typed, put through the same rules as any other.
    // Named in the search box: the batch goes to a task that did not exist a
    // moment ago, without a second dialog.
    if let Some(typed) = choice.strip_prefix(picker::NEW_NAMED) {
        let name = picker::sanitise_workspace(typed);
        if name.is_empty() {
            return Batch::stopped(Outcome::EmptyName { raw: typed.to_string() });
        }
        let results: Vec<(Window, Outcome)> = chosen
            .into_iter()
            .map(|w| {
                let outcome = if wm.move_window(&w.id, &name) {
                    Outcome::Moved { to: name.clone(), created: true }
                } else {
                    Outcome::MoveFailed { to: name.clone() }
                };
                (w, outcome)
            })
            .collect();
        return Batch { target: Some(name), results, aborted: None };
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

    let mut results = Vec::new();
    for window in chosen {
        let outcome = if window.workspace == target {
            Outcome::SameWorkspace
        } else if !wm.move_window(&window.id, &target) {
            Outcome::MoveFailed { to: target.clone() }
        } else {
            let created = !occupancy.contains_key(&target);
            Outcome::Moved { to: target.clone(), created }
        };
        results.push((window, outcome));
    }

    Batch { target: Some(target), results, aborted: None }
}

/// Which list the dialog is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    One,
    Many,
    Jump,
    Board,
}

/// The tabs, with the showing one marked. Built in one place so every mode
/// offers the same strip and cannot drift.
pub fn tabs(showing: Mode) -> Vec<picker::Row> {
    vec![
        picker::Row::tab(TO_ONE, picker::ONCE_LABEL, showing == Mode::One),
        picker::Row::tab(TO_MANY, picker::MANY_LABEL, showing == Mode::Many),
        picker::Row::tab(TO_JUMP, picker::JUMP_LABEL, showing == Mode::Jump),
        picker::Row::tab(TO_BOARD, picker::BOARD_LABEL, showing == Mode::Board),
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
    let occupancy = wm.occupancy();
    let here = wm.focused_workspace().unwrap_or_default();
    let known_to_aerospace = wm.all_workspaces().unwrap_or_default();

    let menu = picker::build_menu(
        occupancy.as_ref(),
        &cfg.known,
        &[cached, &known_to_aerospace].concat(),
        &here,
        &cfg.order.prefixes,
        &cfg.labels,
        pins,
    );
    let mut rows = tabs(Mode::Jump);
    rows.extend(menu.rows.clone());

    let done = |outcome| Run { window: None, occupancy: occupancy.clone(), outcome, brought: Vec::new() };

    let Some(choice) = prompt.choose_rows(&rows, "Jump to a task", "") else {
        return done(Outcome::Cancelled);
    };
    if choice != TO_JUMP && picker::is_mode(&choice) {
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

/// Every window on the machine, grouped by the task it is in.
///
/// macOS groups Mission Control by its own Spaces, which have nothing to do
/// with these: AeroSpace hides a workspace by parking its windows off screen
/// *within* a Space, so Ctrl-↑ shows all of them at once, ungrouped and
/// unlabelled. This draws the same picture from the grouping that was meant.
///
/// Unlike every other mode this one can be answered more than once. A tidy-up
/// is several windows going to several different places, and closing the panel
/// after each would make sorting forty windows forty keypresses — so the drags
/// are collected by the panel and applied together here.
pub fn run_board(
    wm: &dyn WindowManager,
    prompt: &dyn Prompt,
    cfg: &Config,
    cached: &[String],
    follow: Following<'_>,
) -> Session {
    let here = wm.focused_workspace().unwrap_or_default();
    let Some(all) = wm.all_windows() else {
        return Session::Single(Run {
            window: None,
            occupancy: None,
            outcome: Outcome::NoWindow,
            brought: Vec::new(),
        });
    };
    let windows: Vec<Window> = all.into_iter().filter(|w| w.app != DIALOG_APP).collect();
    let occupancy = picker::occupancy_of(&windows);

    let menu = picker::build_window_menu(&windows, &here, |w| follow.has(w));
    let mut rows = tabs(Mode::Board);
    rows.extend(menu.rows.clone());

    // Tasks holding nothing still get a tile, so the board can tidy *into* one
    // that is waiting empty rather than only shuffle between the ones already
    // in use.
    let mut empty: Vec<String> = wm
        .all_workspaces()
        .unwrap_or_default()
        .into_iter()
        .chain(cfg.known.iter().cloned())
        .chain(cached.iter().cloned())
        .filter(|name| !name.is_empty() && !occupancy.contains_key(name))
        .collect();
    empty.sort();
    empty.dedup();
    for name in &empty {
        rows.push(picker::Row::space(name));
    }

    let single = |outcome| {
        Session::Single(Run {
            window: None,
            occupancy: Some(occupancy.clone()),
            outcome,
            brought: Vec::new(),
        })
    };

    let Some(answer) = prompt.choose_board(&rows, "Every window", &here) else {
        return single(Outcome::Cancelled);
    };
    if let Some(tab) = answer.iter().find(|a| *a != TO_BOARD && picker::is_mode(a)) {
        return single(Outcome::SwitchMode { to: tab.clone() });
    }

    // Windows dragged onto tasks, applied together. Each is recorded exactly
    // as a sling is, so tidying on the board feeds `restore` the same
    // statement of intent that slinging by hand does.
    let drags: Vec<(&str, &str)> = answer.iter().filter_map(|a| picker::sling_parts(a)).collect();
    if !drags.is_empty() {
        let mut results = Vec::new();
        for (window_id, target) in drags {
            let Some(window) = windows.iter().find(|w| w.id == window_id) else {
                continue;
            };
            let outcome = if window.workspace == target {
                Outcome::SameWorkspace
            } else if !wm.move_window(&window.id, target) {
                Outcome::MoveFailed { to: target.to_string() }
            } else {
                Outcome::Moved { to: target.to_string(), created: !occupancy.contains_key(target) }
            };
            results.push((window.clone(), outcome));
        }
        // No `follow_to`: a drag moves a window without moving *you*, so there
        // is no arrival for the windows that travel everywhere to catch up to.
        return Session::Batch(Batch { target: None, results, aborted: None });
    }

    let Some(choice) = answer.into_iter().next() else {
        return single(Outcome::Cancelled);
    };

    // A window: go to it. Focusing one that is off screen brings its whole
    // task forward, which is the point — you picked the window, not the task.
    if let Some(window) = windows.iter().find(|w| w.id == choice) {
        if !wm.focus(&window.id) {
            return single(Outcome::RefocusFailed);
        }
        return Session::Single(Run {
            window: Some(window.clone()),
            occupancy: Some(occupancy.clone()),
            outcome: Outcome::Focused { to: window.workspace.clone() },
            brought: Vec::new(),
        });
    }

    // A tile heading: go to the task itself, whether or not it holds anything.
    if occupancy.contains_key(&choice) || empty.contains(&choice) {
        if choice == here {
            return single(Outcome::SameWorkspace);
        }
        if !wm.focus_workspace(&choice) {
            return single(Outcome::MoveFailed { to: choice });
        }
        return single(Outcome::Jumped { to: choice });
    }
    single(Outcome::Heading)
}

/// One keypress, either mode. The dialog carries a row that switches between
/// them, so this reopens rather than returning when that row is picked.
pub fn run_session(
    wm: &dyn WindowManager,
    prompt: &dyn Prompt,
    cfg: &Config,
    cached: &[String],
    follow: Following<'_>,
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
    follow: Following<'_>,
    pins: &[String],
) -> Session {
    let mut mode = start;
    // Toggling is the user's to do as often as they like; the bound is only
    // here so a prompt that always answers "switch" cannot spin forever.
    for _ in 0..16 {
        let switched = |to: &str| match to {
            TO_MANY => Mode::Many,
            TO_JUMP => Mode::Jump,
            TO_BOARD => Mode::Board,
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
            Mode::Board => {
                let session = run_board(wm, prompt, cfg, cached, follow);
                // The board is the one mode that can answer with either shape,
                // so the tab it was left by has to be looked for in both.
                let leaving = match &session {
                    Session::Single(run) => match &run.outcome {
                        Outcome::SwitchMode { to } => Some(to.clone()),
                        _ => None,
                    },
                    Session::Batch(batch) => match &batch.aborted {
                        Some(Outcome::SwitchMode { to }) => Some(to.clone()),
                        _ => None,
                    },
                };
                if let Some(to) = leaving {
                    mode = switched(&to);
                    continue;
                }
                return session;
            }
        }
    }
    Session::Single(Run {
        window: None,
        occupancy: None,
        outcome: Outcome::Cancelled,
        brought: Vec::new(),
    })
}
