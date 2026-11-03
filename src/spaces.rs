//! macOS Spaces as the place a task lives: one Space per task.
//!
//! An experiment — see `docs/SPACES.md`. SkyLight is reached through
//! `slingr-spaces`, a small Swift helper beside this binary, the way
//! `aerospace.rs` reaches AeroSpace through its CLI. What lives here is the one
//! thing macOS has no notion of: which task a Space is for.
//!
//! The limit that shapes all of it: with SIP on, macOS 26 will not let one
//! process move another process's window to a different Space. So this backend
//! switches Spaces and reports what is in them, and windows get where they
//! belong by being opened there — a new window opens on the Space you are in.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::picker::{occupancy_of, Occupancy, Window};
use crate::store::state_dir;
use crate::wm::{run_bounded, WindowManager};

/// Every call is one SkyLight round trip, or a switch that confirms itself
/// within about a second. Anything slower than this is not coming.
const TIMEOUT: Duration = Duration::from_secs(4);

/// Adding a desktop waits on Mission Control opening, a button, and an
/// animation; the helper gives up on its own after nine and a half seconds.
const ADDING: Duration = Duration::from_secs(12);

/// One answer from `slingr-spaces state`: the whole machine, in one call.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct State {
    #[serde(default)]
    pub displays: Vec<Display>,
    #[serde(default)]
    pub windows: Vec<Win>,
    /// The window with keyboard focus, if there is one.
    #[serde(default)]
    pub focused: Option<u64>,
    /// The Space macOS calls active: the one showing on the screen in use.
    #[serde(default)]
    pub active: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Display {
    pub id: String,
    /// The screen with the menu bar. Only its desktops are handed to tasks:
    /// the second screen watches a build, and is not in the rotation.
    #[serde(default)]
    pub main: bool,
    #[serde(default)]
    pub current: u64,
    /// In Mission Control's order, left to right.
    #[serde(default)]
    pub spaces: Vec<Space>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Space {
    pub id: u64,
    #[serde(default)]
    pub uuid: String,
    /// "desktop", or "fullscreen" for the Space a full-screen window makes
    /// for itself — which belongs to that window, and never to a task — or
    /// "other" for anything SkyLight numbers differently.
    #[serde(default, skip_serializing)]
    pub kind: String,
}

impl Space {
    /// The same Space, as far as can be told.
    ///
    /// By uuid where both have one: macOS keeps the uuid in `com.apple.spaces`,
    /// so it is the better bet for surviving a restart. Whether the numeric id
    /// survives one too has not been checked.
    fn is(&self, other: &Space) -> bool {
        if !self.uuid.is_empty() && !other.uuid.is_empty() {
            self.uuid == other.uuid
        } else {
            self.id == other.id
        }
    }

    /// Only a desktop can hold a task. A binding read back from disk carries
    /// no kind, and was a desktop when it was made.
    fn is_desktop(&self) -> bool {
        self.kind.is_empty() || self.kind == "desktop"
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Win {
    pub id: u64,
    #[serde(default)]
    pub app: String,
    #[serde(default)]
    pub bundle: String,
    /// Empty unless whoever launched the helper has Screen Recording.
    #[serde(default)]
    pub title: String,
    /// More than one when the window is on every Space.
    #[serde(default)]
    pub spaces: Vec<u64>,
}

/// Which Space each task lives in.
///
/// macOS has no names for Spaces, so this is the only place a task and a
/// Space ever meet.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bindings {
    #[serde(default)]
    pub tasks: BTreeMap<String, Space>,
}

impl Bindings {
    fn path() -> PathBuf {
        state_dir().join("spaces.json")
    }

    pub fn load() -> Self {
        fs::read_to_string(Self::path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        fs::create_dir_all(state_dir())?;
        fs::write(Self::path(), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Give `task` this Space, taking it from whichever task had it. A Space
    /// is one task's, and a task has one Space.
    pub fn bind(&mut self, task: &str, space: &Space) {
        self.tasks.retain(|t, s| t != task && !s.is(space));
        self.tasks.insert(task.to_string(), space.clone());
    }

    fn task_for(&self, space: &Space) -> Option<&str> {
        self.tasks.iter().find(|(_, s)| s.is(space)).map(|(t, _)| t.as_str())
    }
}

/// The main screen first, then the rest as macOS lists them.
fn screens(state: &State) -> Vec<&Display> {
    let mut out: Vec<&Display> = state.displays.iter().filter(|d| d.main).collect();
    out.extend(state.displays.iter().filter(|d| !d.main));
    out
}

/// What each Space is called: the task it was given, or where it sits —
/// `desktop-3` on the main screen, `screen-2-desktop-1` on another.
///
/// Desktops are counted apart from full-screen Spaces because Mission Control
/// counts them that way, so `desktop-3` is the one it labels Desktop 3.
pub fn names(state: &State, bindings: &Bindings) -> BTreeMap<u64, String> {
    let mut out = BTreeMap::new();
    for (screen, display) in screens(state).into_iter().enumerate() {
        let prefix = if screen == 0 { String::new() } else { format!("screen-{}-", screen + 1) };
        let (mut desktops, mut others) = (0, 0);
        for space in &display.spaces {
            let place = if space.is_desktop() {
                desktops += 1;
                format!("{prefix}desktop-{desktops}")
            } else {
                others += 1;
                format!("{prefix}{}-{others}", space.kind)
            };
            let name = bindings.task_for(space).map(str::to_string).unwrap_or(place);
            out.insert(space.id, name);
        }
    }
    out
}

/// What the agent writes over Mission Control's desktops, since macOS will
/// not let one be renamed. What `slingr labels` prints.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Labels {
    pub bars: Vec<Bar>,
}

/// One screen's row of Spaces along the top of Mission Control.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Bar {
    /// As SkyLight names the screen: its uuid, or "Main" when every screen
    /// shares one set of Spaces.
    pub display: String,
    pub main: bool,
    /// One per Space, in Mission Control's order: the task a desktop is for,
    /// or nothing, where Mission Control's own name is the better one.
    pub names: Vec<Option<String>>,
}

/// Only a task's own name: `desktop-3` written over "Desktop 3" would say
/// nothing new, and a full-screen window's Space is already called by its
/// application.
pub fn labels(state: &State, bindings: &Bindings) -> Labels {
    let bars = state
        .displays
        .iter()
        .map(|d| Bar {
            display: d.id.clone(),
            main: d.main,
            names: d
                .spaces
                .iter()
                .map(|s| if s.is_desktop() { bindings.task_for(s).map(str::to_string) } else { None })
                .collect(),
        })
        .collect();
    Labels { bars }
}

/// The Space a task lives in, if it has one that still exists. A place name
/// such as `desktop-2` answers too, so an unclaimed desktop can be gone to.
pub fn space_of(task: &str, state: &State, bindings: &Bindings) -> Option<u64> {
    names(state, bindings).into_iter().find(|(_, name)| name == task).map(|(id, _)| id)
}

/// A desktop on the main screen that no task has claimed and nothing lives in
/// — where a task with no Space yet can begin.
///
/// A window on every Space, such as a terminal assigned to All Desktops, does
/// not count as living in it: it is there whichever desktop this turns out to
/// be.
pub fn free_space(state: &State, bindings: &Bindings) -> Option<Space> {
    let main = screens(state).into_iter().next()?;
    main.spaces
        .iter()
        .filter(|s| s.is_desktop())
        .filter(|s| bindings.task_for(s).is_none())
        .find(|s| !state.windows.iter().any(|w| w.spaces == [s.id]))
        .cloned()
}

/// How to get to a task.
#[derive(Debug, PartialEq, Eq)]
pub enum Way {
    /// It has a desktop, or names one.
    Go(u64),
    /// Its first visit, and an empty desktop is waiting for it.
    Claim(Space),
    /// Its first visit, with every desktop in use: a new one, which only the
    /// agent can make.
    Add,
}

pub fn way_to(task: &str, state: &State, bindings: &Bindings) -> Way {
    if let Some(id) = space_of(task, state, bindings) {
        return Way::Go(id);
    }
    match free_space(state, bindings) {
        Some(space) => Way::Claim(space),
        None => Way::Add,
    }
}

/// What `slingr-spaces add` answers: the desktop, or why there is none.
#[derive(Debug, Default, Deserialize)]
struct Added {
    #[serde(default)]
    space: Option<Space>,
    #[serde(default)]
    refused: Option<String>,
}

fn added(answer: &str) -> Result<Space, String> {
    match serde_json::from_str::<Added>(answer) {
        Ok(Added { space: Some(space), .. }) => Ok(space),
        Ok(Added { refused: Some(why), .. }) => Err(why),
        _ => Err("the helper gave no answer".into()),
    }
}

/// Which Space a window counts as being in. One on every Space is wherever
/// you are; otherwise the one it is on.
fn placed_on(w: &Win, active: u64) -> u64 {
    if w.spaces.len() > 1 && w.spaces.contains(&active) {
        active
    } else {
        w.spaces.first().copied().unwrap_or_default()
    }
}

fn display_of(state: &State, space: u64) -> Option<&Display> {
    state.displays.iter().find(|d| d.spaces.iter().any(|s| s.id == space))
}

fn window(w: &Win, state: &State, names: &BTreeMap<u64, String>) -> Window {
    let space = placed_on(w, state.active);
    Window {
        id: w.id.to_string(),
        workspace: names.get(&space).cloned().unwrap_or_default(),
        monitor: display_of(state, space).map(|d| d.id.clone()).unwrap_or_default(),
        app: w.app.clone(),
        title: w.title.clone(),
        bundle: w.bundle.clone(),
    }
}

/// Said through Notification Centre, because the one flow that needs it runs
/// from a hotkey or a herdr hook, where nothing else would ever be seen.
fn notify(message: &str) {
    let script = format!(
        "display notification \"{}\" with title \"slingr\"",
        crate::dialog::applescript_string(message)
    );
    let _ = Command::new("osascript").arg("-e").arg(script).output();
}

pub struct Spaces {
    helper: PathBuf,
}

impl Default for Spaces {
    fn default() -> Self {
        Self { helper: crate::panel::beside("slingr-spaces") }
    }
}

impl Spaces {
    fn ask(&self, args: &[&str]) -> Option<String> {
        run_bounded(&self.helper.to_string_lossy(), args, TIMEOUT).ok()
    }

    pub fn state(&self) -> Option<State> {
        serde_json::from_str(&self.ask(&["state"])?).ok()
    }

    pub fn helper(&self) -> &PathBuf {
        &self.helper
    }

    /// Give the Space showing on the main screen to `task`.
    ///
    /// How an existing desktop — the one everything is already sitting in,
    /// say — becomes a task, rather than waiting for a task to claim an empty
    /// one.
    pub fn bind_here(&self, task: &str) -> Result<String> {
        let Some(state) = self.state() else { bail!("{} did not answer", self.helper.display()) };
        let Some(main) = screens(&state).into_iter().next() else { bail!("no screen reported") };
        let Some(space) = main.spaces.iter().find(|s| s.id == main.current) else {
            bail!("the main screen's current Space is not in its own list")
        };
        if !space.is_desktop() {
            bail!("that is a full-screen window's Space, which belongs to the window");
        }
        let mut bindings = Bindings::load();
        let was = names(&state, &bindings).get(&space.id).cloned().unwrap_or_default();
        bindings.bind(task, space);
        bindings.save()?;
        self.tell_agent();
        Ok(was)
    }

    /// A new desktop on the main screen, made by the agent through Mission
    /// Control, or why there is none.
    fn add_desktop(&self) -> Result<Space, String> {
        match run_bounded(&self.helper.to_string_lossy(), &["add"], ADDING) {
            Ok(answer) => added(&answer),
            Err(_) => Err(format!("{} did not answer", self.helper.display())),
        }
    }

    /// The agent's menu bar names the task in front. A desktop that has just
    /// become a task's has not moved, so nothing else would tell it.
    fn tell_agent(&self) {
        let _ = self.ask(&["refresh"]);
    }

    /// Show `space`, unless it already is.
    fn show(&self, space: u64, state: &State) -> bool {
        if display_of(state, space).is_some_and(|d| d.current == space) {
            return true;
        }
        self.ask(&["switch", &space.to_string()]).is_some()
    }
}

impl WindowManager for Spaces {
    fn name(&self) -> &str {
        "Spaces"
    }

    fn moves_windows(&self) -> bool {
        false
    }

    fn focused_window(&self) -> Option<Window> {
        let state = self.state()?;
        let id = state.focused?;
        let names = names(&state, &Bindings::load());
        state.windows.iter().find(|w| w.id == id).map(|w| window(w, &state, &names))
    }

    fn all_windows(&self) -> Option<Vec<Window>> {
        let state = self.state()?;
        let names = names(&state, &Bindings::load());
        Some(state.windows.iter().map(|w| window(w, &state, &names)).collect())
    }

    fn occupancy(&self) -> Option<BTreeMap<String, Occupancy>> {
        Some(occupancy_of(&self.all_windows()?))
    }

    /// Goes to the window's Space and brings its application forward. Which
    /// of that application's windows ends up in front is the application's
    /// choice unless the helper has Accessibility, which lets it raise the one
    /// that was asked for.
    fn focus(&self, window_id: &str) -> bool {
        self.ask(&["focus", window_id]).is_some()
    }

    fn move_focused_to(&self, _workspace: &str) -> bool {
        false
    }

    /// A task's first visit claims an empty desktop, or has one made, so going
    /// to a task that has never been anywhere is the same gesture as going to
    /// one that has.
    fn focus_workspace(&self, workspace: &str) -> bool {
        let Some(state) = self.state() else { return false };
        let mut bindings = Bindings::load();
        let space = match way_to(workspace, &state, &bindings) {
            Way::Go(id) => return self.show(id, &state),
            Way::Claim(space) => space,
            Way::Add => match self.add_desktop() {
                Ok(space) => space,
                Err(why) => {
                    notify(&format!("No desktop for {workspace}: {why}."));
                    return false;
                }
            },
        };
        bindings.bind(workspace, &space);
        if bindings.save().is_err() {
            return false;
        }
        self.tell_agent();
        self.show(space.id, &state)
    }

    fn move_window(&self, _window_id: &str, _workspace: &str) -> bool {
        false
    }

    fn focused_monitor(&self) -> Option<String> {
        let state = self.state()?;
        display_of(&state, state.active).map(|d| d.id.clone())
    }

    fn focused_workspace(&self) -> Option<String> {
        let state = self.state()?;
        names(&state, &Bindings::load()).get(&state.active).cloned()
    }

    fn all_workspaces(&self) -> Option<Vec<String>> {
        let state = self.state()?;
        Some(names(&state, &Bindings::load()).into_values().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space(id: u64, uuid: &str) -> Space {
        Space { id, uuid: uuid.into(), kind: "desktop".into() }
    }

    fn fullscreen(id: u64) -> Space {
        Space { id, uuid: format!("fs-{id}"), kind: "fullscreen".into() }
    }

    fn win(id: u64, spaces: &[u64]) -> Win {
        Win { id, app: "Brave Browser".into(), bundle: "com.brave.Browser".into(), title: String::new(), spaces: spaces.to_vec() }
    }

    /// The main screen with three desktops and a full-screen window between
    /// the second and third, and a second screen with one desktop. Listed
    /// second screen first, as macOS may.
    fn machine() -> State {
        State {
            displays: vec![
                Display { id: "XDR".into(), main: false, current: 9, spaces: vec![space(9, "u9")] },
                Display {
                    id: "MAIN".into(),
                    main: true,
                    current: 3,
                    spaces: vec![space(3, "u3"), space(1310, "u1310"), fullscreen(77), space(1400, "u1400")],
                },
            ],
            windows: vec![win(1, &[3]), win(2, &[3]), win(3, &[9]), win(4, &[77])],
            focused: Some(1),
            active: 3,
        }
    }

    fn bound(pairs: &[(&str, Space)]) -> Bindings {
        Bindings { tasks: pairs.iter().map(|(t, s)| (t.to_string(), s.clone())).collect() }
    }

    #[test]
    fn unclaimed_spaces_are_named_the_way_mission_control_numbers_them() {
        let names = names(&machine(), &Bindings::default());
        assert_eq!(names[&3], "desktop-1");
        assert_eq!(names[&1310], "desktop-2");
        // A full-screen window's Space is not a desktop, and does not shift
        // the numbering of the desktops after it.
        assert_eq!(names[&77], "fullscreen-1");
        assert_eq!(names[&1400], "desktop-3");
        // The main screen is numbered first however macOS lists the screens.
        assert_eq!(names[&9], "screen-2-desktop-1");
    }

    #[test]
    fn a_claimed_space_is_called_by_its_task() {
        let b = bound(&[("t-forms", space(1310, "u1310"))]);
        assert_eq!(names(&machine(), &b)[&1310], "t-forms");
        assert_eq!(space_of("t-forms", &machine(), &b), Some(1310));
    }

    #[test]
    fn a_space_is_recognised_by_its_uuid_even_if_its_number_changed() {
        // Whether the numbers survive a restart is unknown, so they are not
        // what a task holds on to.
        let b = bound(&[("t-forms", space(55, "u1310"))]);
        assert_eq!(space_of("t-forms", &machine(), &b), Some(1310));
    }

    #[test]
    fn a_space_reported_without_a_uuid_is_recognised_by_its_number() {
        let mut state = machine();
        state.displays[1].spaces[1].uuid.clear();
        let b = bound(&[("t-forms", space(1310, ""))]);
        assert_eq!(space_of("t-forms", &state, &b), Some(1310));
    }

    #[test]
    fn a_task_whose_desktop_was_removed_has_nowhere_to_go_yet() {
        let b = bound(&[("t-forms", space(2000, "gone"))]);
        assert_eq!(space_of("t-forms", &machine(), &b), None);
    }

    #[test]
    fn mission_control_is_given_the_names_of_tasks_and_nothing_else() {
        let b = bound(&[("t-forms", space(1310, "u1310"))]);
        let Labels { bars } = labels(&machine(), &b);
        // Every screen, each in Mission Control's order, one name per Space —
        // the agent matches them up by position, so none can be left out.
        assert_eq!(bars[0], Bar { display: "XDR".into(), main: false, names: vec![None] });
        assert_eq!(
            bars[1],
            Bar { display: "MAIN".into(), main: true, names: vec![None, Some("t-forms".into()), None, None] }
        );

        // A binding with no uuid is matched by number, and a full-screen
        // window's Space with that number is still not given a task's name.
        let odd = bound(&[("t-forms", Space { id: 77, uuid: String::new(), kind: String::new() })]);
        assert_eq!(labels(&machine(), &odd).bars[1].names, vec![None; 4]);
    }

    /// The agent decodes these in Swift, where a renamed field is a name that
    /// silently never appears.
    #[test]
    fn the_agent_reads_labels_the_way_this_writes_them() {
        let agent = include_str!("../agent/main.swift");
        for field in ["bars", "display", "main", "names"] {
            assert!(agent.contains(&format!("let {field}:")), "the agent never decodes `{field}`");
        }
        assert!(agent.contains(r#"runAndWait(["labels"])"#), "the agent never asks for labels");
        assert!(include_str!("main.rs").contains("Some(Cmd::Labels) =>"), "slingr has no `labels` command");

        let b = bound(&[("t-forms", space(1310, "u1310"))]);
        let json = serde_json::to_string(&labels(&machine(), &b)).unwrap();
        assert!(json.starts_with(r#"{"bars":[{"display":"XDR","main":false,"names":[null]}"#), "{json}");
    }

    #[test]
    fn a_place_name_goes_to_that_desktop() {
        assert_eq!(space_of("desktop-3", &machine(), &Bindings::default()), Some(1400));
        assert_eq!(space_of("desktop-9", &machine(), &Bindings::default()), None);
    }

    #[test]
    fn a_new_task_gets_the_first_empty_unclaimed_desktop_on_the_main_screen() {
        // desktop-1 holds windows, so the first free one is desktop-2.
        assert_eq!(free_space(&machine(), &Bindings::default()).map(|s| s.id), Some(1310));

        // Once desktop-2 is claimed, the next is past the full-screen Space.
        let b = bound(&[("t-forms", space(1310, "u1310"))]);
        assert_eq!(free_space(&machine(), &b).map(|s| s.id), Some(1400));

        // And with every desktop taken there is nowhere, rather than the
        // second screen or a full-screen window's Space.
        let b = bound(&[("t-forms", space(1310, "u1310")), ("t-mail", space(1400, "u1400"))]);
        assert_eq!(free_space(&machine(), &b), None);
    }

    #[test]
    fn a_task_with_every_desktop_taken_has_one_made_rather_than_nowhere_to_go() {
        let full = bound(&[("t-forms", space(1310, "u1310")), ("t-mail", space(1400, "u1400"))]);
        assert_eq!(way_to("t-arc", &machine(), &full), Way::Add);

        // Never while it has one, or while an empty one is waiting.
        assert_eq!(way_to("t-forms", &machine(), &full), Way::Go(1310));
        assert_eq!(way_to("t-arc", &machine(), &Bindings::default()), Way::Claim(space(1310, "u1310")));
    }

    #[test]
    fn the_helper_answers_add_with_the_desktop_or_the_reason_there_is_none() {
        let made = r#"{"space":{"id":1500,"uuid":"u1500","kind":"desktop"}}"#;
        assert_eq!(added(made), Ok(space(1500, "u1500")));
        assert_eq!(
            added(r#"{"refused":"slingr agent needs Accessibility"}"#),
            Err("slingr agent needs Accessibility".to_string())
        );
        // The reason is what the notification says, so silence must not
        // pass for a desktop.
        assert!(added("").is_err());
        assert!(added("{}").is_err());
    }

    /// The helper and the agent are separate programs that find each other by
    /// these names alone, and a misspelling in either is silence rather than
    /// an error.
    #[test]
    fn the_helper_and_the_agent_call_each_other_the_same_names() {
        let helper = include_str!("../spaces/main.swift");
        let agent = include_str!("../agent/main.swift");
        for name in [
            "dev.slingr.agent.add-desktop",
            "dev.slingr.agent.add-desktop.refused",
            "dev.slingr.agent.switch-desktop",
            "dev.slingr.agent.switch-desktop.refused",
            "dev.slingr.agent.refresh",
        ] {
            assert!(helper.contains(&format!("\"{name}\"")), "slingr-spaces never uses {name}");
            assert!(agent.contains(&format!("\"{name}\"")), "the agent never uses {name}");
        }
        // libc's `pause()` takes nothing and waits for a signal, so a Swift
        // `pause()` meaning anything else calls it instead — and the helper
        // waits for ever, which is how the first build of this hung.
        assert!(!helper.contains("pause()"), "slingr-spaces calls libc's pause(), which never returns");

        // The helper looks for the agent by its bundle identifier before
        // asking, so that a missing agent is said at once.
        assert!(helper.contains(r#"bundle = "dev.slingr.agent""#));
        assert!(include_str!("../agent/Info.plist").contains("<string>dev.slingr.agent</string>"));
    }

    #[test]
    fn a_window_on_every_desktop_does_not_make_one_occupied() {
        // A terminal assigned to All Desktops is on all of them, and would
        // otherwise leave no desktop ever looking empty.
        let mut state = machine();
        state.windows.push(win(5, &[3, 1310, 1400]));
        assert_eq!(free_space(&state, &Bindings::default()).map(|s| s.id), Some(1310));
    }

    #[test]
    fn a_window_on_every_desktop_counts_as_where_you_are() {
        assert_eq!(placed_on(&win(5, &[3, 1310, 1400]), 1310), 1310);
        assert_eq!(placed_on(&win(6, &[1400]), 1310), 1400);
    }

    #[test]
    fn claiming_a_space_takes_it_from_its_previous_task_and_moves_the_task() {
        let mut b = bound(&[("t-forms", space(1310, "u1310")), ("t-mail", space(1400, "u1400"))]);
        b.bind("t-mail", &space(1310, "u1310"));
        // One Space, one task — and the task is no longer on its old desktop.
        assert_eq!(b, bound(&[("t-mail", space(1310, "u1310"))]));
    }

    #[test]
    fn windows_report_the_task_and_screen_they_are_on() {
        let b = bound(&[("infra", space(3, "u3"))]);
        let state = machine();
        let names = names(&state, &b);
        let w = window(&state.windows[0], &state, &names);
        assert_eq!((w.workspace.as_str(), w.monitor.as_str()), ("infra", "MAIN"));
        let elsewhere = window(&state.windows[2], &state, &names);
        assert_eq!((elsewhere.workspace.as_str(), elsewhere.monitor.as_str()), ("screen-2-desktop-1", "XDR"));
    }

    /// The helper encodes these in Swift, where a renamed field is not a
    /// compile error but a value that silently never arrives — every field
    /// here has a default. Pin the spelling against the Swift source itself.
    #[test]
    fn the_helper_spells_its_answer_the_way_this_reads_it() {
        let swift = include_str!("../spaces/main.swift");
        for field in [
            "displays", "windows", "focused", "active", // State
            "id", "main", "current", "spaces", // Display
            "uuid", "kind", // Space
            "app", "bundle", "title", // Win
            "space", "refused", // Added
        ] {
            assert!(
                swift.contains(&format!("let {field}:")),
                "slingr-spaces never encodes `{field}`"
            );
        }

        let answer = r#"{"displays":[{"id":"MAIN","main":true,"current":3,
            "spaces":[{"id":3,"uuid":"u3","kind":"desktop"}]}],
            "windows":[{"id":11,"pid":42,"app":"Brave Browser","bundle":"com.brave.Browser",
            "title":"","spaces":[3]}],"focused":11,"active":3}"#;
        let state: State = serde_json::from_str(answer).expect("decodes");
        assert_eq!(state.displays[0].spaces[0], space(3, "u3"));
        assert_eq!(state.windows[0].spaces, vec![3]);
        assert_eq!((state.focused, state.active), (Some(11), 3));
    }
}
