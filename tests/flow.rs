//! The sling flow, driven without AeroSpace or a screen.

use std::cell::RefCell;
use std::collections::BTreeMap;

use sling::aerospace::WindowManager;
use sling::app::{self, Outcome};
use sling::config::{Config, Order};
use sling::dialog::Prompt;
use sling::app::Session;
use sling::picker::{Row, Window, ALL, NEW, TO_MANY, TO_ONE};

#[derive(Debug, PartialEq, Eq)]
enum Call {
    Focus(String),
    Move(String),
    /// A move that names its window instead of focusing it.
    MoveById(String, String),
}

struct FakeWm {
    window: Option<Window>,
    all: Vec<Window>,
    counts: Option<BTreeMap<String, usize>>,
    focus_succeeds: bool,
    /// Window ids that refuse to take focus, however hard you ask — a
    /// minimised window behaves exactly like this.
    focus_refuses: Vec<String>,
    move_succeeds: bool,
    calls: RefCell<Vec<Call>>,
}

fn window(id: &str, workspace: &str, title: &str) -> Window {
    Window {
        id: id.into(),
        workspace: workspace.into(),
        monitor: "1".into(),
        app: "Brave Browser".into(),
        bundle: "com.brave.Browser".into(),
        title: title.into(),
    }
}

/// The same window, on the other screen.
fn window_on(id: &str, workspace: &str, title: &str, monitor: &str) -> Window {
    Window { monitor: monitor.into(), ..window(id, workspace, title) }
}

impl FakeWm {
    fn new() -> Self {
        Self {
            window: Some(window("11513", "infra", "Arty Corner Mail")),
            all: vec![
                window("11513", "infra", "Arty Corner Mail"),
                // Two windows, one title. The real window list has several
                // such pairs, so rows cannot be addressed by their text.
                window("7695", "infra", "The Framing Queue"),
                window("7686", "infra", "The Framing Queue"),
                window("19369", "t-mail", "task-title-change"),
            ],
            counts: Some(BTreeMap::from([
                ("infra".to_string(), 3usize),
                ("ac-app".to_string(), 2usize),
            ])),
            focus_succeeds: true,
            focus_refuses: Vec::new(),
            move_succeeds: true,
            calls: RefCell::new(Vec::new()),
        }
    }
}

impl WindowManager for FakeWm {
    fn focused_window(&self) -> Option<Window> {
        self.window.clone()
    }
    fn window_counts(&self) -> Option<BTreeMap<String, usize>> {
        self.counts.clone()
    }
    fn all_windows(&self) -> Option<Vec<Window>> {
        Some(self.all.clone())
    }
    fn focus(&self, window_id: &str) -> bool {
        self.calls.borrow_mut().push(Call::Focus(window_id.into()));
        self.focus_succeeds && !self.focus_refuses.iter().any(|id| id == window_id)
    }
    fn move_focused_to(&self, workspace: &str) -> bool {
        self.calls.borrow_mut().push(Call::Move(workspace.into()));
        self.move_succeeds
    }
    fn focus_workspace(&self, _workspace: &str) -> bool {
        true
    }
    fn focused_monitor(&self) -> Option<String> {
        self.window.as_ref().map(|w| w.monitor.clone())
    }
    fn all_workspaces(&self) -> Option<Vec<String>> {
        self.counts.as_ref().map(|c| c.keys().cloned().collect())
    }
    fn move_window(&self, window_id: &str, workspace: &str) -> bool {
        self.calls.borrow_mut().push(Call::MoveById(window_id.into(), workspace.into()));
        self.move_succeeds && !self.focus_refuses.iter().any(|id| id == window_id)
    }
}

struct FakePrompt {
    choice: Option<String>,
    /// Answers given in order, when a flow asks more than once.
    choices: RefCell<std::collections::VecDeque<String>>,
    text: Option<String>,
    /// Row numbers to select in a multi-selection dialog.
    many: Vec<usize>,
}

impl FakePrompt {
    /// Pick the menu line ending in `name`, the way a human picks a label
    /// rather than a workspace id.
    fn picking(name: &str) -> Self {
        Self {
            choice: Some(name.into()),
            choices: RefCell::new(Default::default()),
            text: None,
            many: Vec::new(),
        }
    }

    /// Take every window, select those numbered rows, then send them to
    /// `target`. The first answer is the pile picker.
    fn selecting(rows: &[usize], target: &str) -> Self {
        Self {
            choices: RefCell::new(Default::default()),
            choice: Some(target.into()),
            text: None,
            many: rows.to_vec(),
        }
    }
}

impl Prompt for FakePrompt {
    // The flat-string methods are the AppleScript fallback's business; the
    // flow asks the row questions, so those are what the fake answers.
    fn choose(&self, _: &[String], _: &str, _: &str) -> Option<String> {
        None
    }
    fn choose_many(&self, _: &[String], _: &str, _: &str) -> Option<Vec<String>> {
        None
    }
    fn choose_rows(&self, rows: &[Row], _t: &str, _p: &str) -> Option<String> {
        let queued = self.choices.borrow_mut().pop_front();
        let wanted = queued.or_else(|| self.choice.clone())?;
        rows.iter()
            .find(|r| r.id == wanted || r.label == wanted || r.label.ends_with(wanted.as_str()))
            .map(|r| r.id.clone())
    }
    fn choose_many_rows(&self, rows: &[Row], _t: &str, _p: &str) -> Option<Vec<String>> {
        if self.many.is_empty() {
            return None;
        }
        // Row numbers as a human would count them: 1 is the first real entry,
        // skipping the action row at the top.
        let picked: Vec<String> =
            self.many.iter().filter_map(|n| selectable(rows).get(n - 1).map(|r| r.id.clone())).collect();
        (!picked.is_empty()).then_some(picked)
    }
    fn ask_text(&self, _title: &str, _prompt: &str) -> Option<String> {
        self.text.clone()
    }
}

/// Rows a person could actually choose: not the tabs, not the action rows.
fn selectable(rows: &[Row]) -> Vec<&Row> {
    rows.iter()
        .filter(|r| !matches!(r.marker.as_deref(), Some("action") | Some("tab")))
        .collect()
}

/// Pick the row a human would: by the workspace name, whatever decoration the
/// menu has put around it (a count, the "here now" note, indentation).
fn matches_row(item: &str, wanted: &str) -> bool {
    let row = item.trim();
    row == wanted
        || row.starts_with(&format!("{wanted}  ("))
        || row.ends_with(wanted)
}

fn config() -> Config {
    Config {
        known: vec!["ac-app".into(), "t-forms".into(), "infra".into(), "t-mail".into()],
        labels: BTreeMap::from([("ac".to_string(), "art corner".to_string())]),
        order: Order { prefixes: vec!["t".into(), "ac".into()] },
    }
}

#[test]
fn moves_the_window_it_was_asked_about() {
    let wm = FakeWm::new();
    let run = app::run(&wm, &FakePrompt::picking("ac-app"), &config(), &[], &[], &[]);

    assert_eq!(run.outcome, Outcome::Moved { to: "ac-app".into(), created: false });
    // Focus must come first: move-node-to-workspace acts on whatever is
    // focused, so the order is the correctness property, not a detail.
    assert_eq!(
        *wm.calls.borrow(),
        vec![Call::MoveById("11513".into(), "ac-app".into())]
    );
}

#[test]
fn the_move_names_its_window_and_never_touches_focus() {
    // Once the regression test for a terminal that got slung by mistake: the
    // dialog stole focus, and `move-node-to-workspace` moved whatever had
    // inherited it. Naming the window removes the failure entirely rather than
    // guarding against it, so this now asserts the stronger property — no
    // focus call is made at all, so there is nothing to steal.
    let mut wm = FakeWm::new();
    wm.focus_succeeds = false;
    let run = app::run(&wm, &FakePrompt::picking("ac-app"), &config(), &[], &[], &[]);

    assert_eq!(run.outcome, Outcome::Moved { to: "ac-app".into(), created: false });
    assert_eq!(*wm.calls.borrow(), vec![Call::MoveById("11513".into(), "ac-app".into())]);
    assert!(
        !wm.calls.borrow().iter().any(|c| matches!(c, Call::Focus(_))),
        "moving should not involve focus"
    );
}

#[test]
fn slings_to_a_task_that_holds_nothing_yet() {
    // The whole point of a known-but-empty workspace: starting a new task.
    // These entries are indented for alignment, and the indent is lost on the
    // way back from the dialog — so the lookup has to survive that.
    let wm = FakeWm::new();
    let run = app::run(&wm, &FakePrompt::picking("t-forms"), &config(), &[], &[], &[]);

    assert_eq!(run.outcome, Outcome::Moved { to: "t-forms".into(), created: true });
    assert_eq!(*wm.calls.borrow(), vec![Call::MoveById("11513".into(), "t-forms".into())]);
}

#[test]
fn refuses_to_sling_its_own_dialog() {
    // Pressing the key twice running: System Events still holds focus after
    // the first dialog closes, so this is what the second press sees.
    let mut wm = FakeWm::new();
    wm.window = Some(Window {
        id: "19668".into(),
        workspace: "infra".into(),
        monitor: "1".into(),
        app: "System Events".into(),
        bundle: "com.apple.systemevents".into(),
        title: String::new(),
    });
    let run = app::run(&wm, &FakePrompt::picking("t-forms"), &config(), &[], &[], &[]);

    assert_eq!(run.outcome, Outcome::OwnDialog);
    assert!(wm.calls.borrow().is_empty());
}

#[test]
fn a_failed_move_is_reported_rather_than_assumed() {
    let mut wm = FakeWm::new();
    wm.move_succeeds = false;
    let run = app::run(&wm, &FakePrompt::picking("ac-app"), &config(), &[], &[], &[]);
    assert_eq!(run.outcome, Outcome::MoveFailed { to: "ac-app".into() });
}

#[test]
fn cancelling_moves_nothing() {
    let wm = FakeWm::new();
    let run = app::run(&wm, &FakePrompt { choice: None, choices: RefCell::new(Default::default()), text: None, many: Vec::new() }, &config(), &[], &[], &[]);
    assert_eq!(run.outcome, Outcome::Cancelled);
    assert!(wm.calls.borrow().is_empty());
}

#[test]
fn a_choice_that_names_no_task_moves_nothing() {
    // Section headings are not rows, so they cannot be picked; this guards the
    // remaining case — an answer that matches no row at all.
    let wm = FakeWm::new();
    let prompt = FakePrompt { choice: Some("not-a-workspace".into()), choices: RefCell::new(Default::default()), text: None, many: Vec::new() };
    let run = app::run(&wm, &prompt, &config(), &[], &[], &[]);
    assert_eq!(run.outcome, Outcome::Cancelled);
    assert!(wm.calls.borrow().is_empty());
}

#[test]
fn slinging_to_where_it_already_is_moves_nothing() {
    let wm = FakeWm::new();
    let run = app::run(&wm, &FakePrompt::picking("infra"), &config(), &[], &[], &[]);
    assert_eq!(run.outcome, Outcome::SameWorkspace);
    assert!(wm.calls.borrow().is_empty());
}

#[test]
fn a_new_workspace_is_named_the_way_herdr_names_tabs() {
    let wm = FakeWm::new();
    let prompt = FakePrompt { choice: Some(NEW.into()), choices: RefCell::new(Default::default()), text: Some("t/forms".into()), many: Vec::new() };
    let run = app::run(&wm, &prompt, &config(), &[], &[], &[]);

    // "/" would hang AeroSpace on a modal, so the reflex spelling is accepted
    // and translated rather than rejected.
    assert_eq!(run.outcome, Outcome::Moved { to: "t-forms".into(), created: true });
    assert_eq!(*wm.calls.borrow(), vec![Call::MoveById("11513".into(), "t-forms".into())]);
}

#[test]
fn a_name_that_sanitises_away_moves_nothing() {
    let wm = FakeWm::new();
    let prompt = FakePrompt { choice: Some(NEW.into()), choices: RefCell::new(Default::default()), text: Some("///".into()), many: Vec::new() };
    let run = app::run(&wm, &prompt, &config(), &[], &[], &[]);
    assert_eq!(run.outcome, Outcome::EmptyName { raw: "///".into() });
    assert!(wm.calls.borrow().is_empty());
}

#[test]
fn nothing_focused_is_not_an_error() {
    let mut wm = FakeWm::new();
    wm.window = None;
    let run = app::run(&wm, &FakePrompt::picking("ac-app"), &config(), &[], &[], &[]);
    assert_eq!(run.outcome, Outcome::NoWindow);
    assert!(run.window.is_none());
}

#[test]
fn a_silent_aerospace_still_gives_a_usable_menu() {
    // Workspaces remembered from previous runs keep the menu populated, and
    // "created" stays false because we cannot know what is live.
    let mut wm = FakeWm::new();
    wm.counts = None;
    let cached = vec!["ac-shopify".into()];
    let run = app::run(&wm, &FakePrompt::picking("ac-shopify"), &config(), &cached, &[], &[]);

    assert_eq!(run.outcome, Outcome::Moved { to: "ac-shopify".into(), created: false });
    assert!(run.counts.is_none());
}

// --- selecting several windows at once -------------------------------------

#[test]
fn sends_every_selected_window_to_one_task() {
    let wm = FakeWm::new();
    // Rows 1-3 are the infra windows: the drained workspace is listed first.
    let batch = app::run_many(&wm, &FakePrompt::selecting(&[1, 2, 3], "t-forms"), &config(), &[], &[], &[]);

    assert_eq!(batch.target.as_deref(), Some("t-forms"));
    assert_eq!(batch.moved(), 3);
    assert_eq!(
        *wm.calls.borrow(),
        vec![
            Call::MoveById("11513".into(), "t-forms".into()),
            Call::MoveById("7695".into(), "t-forms".into()),
            Call::MoveById("7686".into(), "t-forms".into()),
        ]
    );
}

#[test]
fn shows_the_id_only_where_titles_collide() {
    let wm = FakeWm::new();
    let menu = sling::picker::build_window_menu(&wm.all, "infra", &[]);
    let shown = menu.items.join("\n");
    assert!(shown.contains("The Framing Queue  [7686]"), "{shown}");
    assert!(shown.contains("The Framing Queue  [7695]"), "{shown}");
    // A unique title is left clean.
    assert!(shown.contains("Arty Corner Mail"));
    assert!(!shown.contains("Arty Corner Mail  ["), "{shown}");
}

#[test]
fn addresses_windows_by_row_not_by_title() {
    // 7695 and 7686 share a title. Selecting one row must move that one window.
    let wm = FakeWm::new();
    let batch = app::run_many(&wm, &FakePrompt::selecting(&[2], "t-forms"), &config(), &[], &[], &[]);

    assert_eq!(batch.moved(), 1);
    let (moved, _) = &batch.results[0];
    assert_eq!(moved.title, "The Framing Queue");
    assert_eq!(*wm.calls.borrow(), vec![Call::MoveById("7686".into(), "t-forms".into())]);
}

#[test]
fn an_unreachable_window_does_not_strand_the_rest() {
    let mut wm = FakeWm::new();
    wm.focus_refuses = vec!["7686".into()];
    let batch = app::run_many(&wm, &FakePrompt::selecting(&[1, 2, 3], "t-forms"), &config(), &[], &[], &[]);

    assert_eq!(batch.moved(), 2);
    let skipped: Vec<&str> = batch
        .results
        .iter()
        .filter(|(_, o)| matches!(o, Outcome::MoveFailed { .. }))
        .map(|(w, _)| w.id.as_str())
        .collect();
    assert_eq!(skipped, vec!["7686"]);
}

#[test]
fn a_window_already_in_the_target_is_left_alone() {
    let wm = FakeWm::new();
    // Row 4 is the window already sitting in t-mail.
    let batch = app::run_many(&wm, &FakePrompt::selecting(&[4], "t-mail"), &config(), &[], &[], &[]);

    assert_eq!(batch.moved(), 0);
    assert_eq!(batch.results[0].1, Outcome::SameWorkspace);
    assert!(wm.calls.borrow().is_empty());
}

#[test]
fn selecting_nothing_moves_nothing() {
    let wm = FakeWm::new();
    let batch = app::run_many(&wm, &FakePrompt::selecting(&[], "t-forms"), &config(), &[], &[], &[]);
    assert_eq!(batch.aborted, Some(Outcome::Cancelled));
    assert!(wm.calls.borrow().is_empty());
}

// --- switching modes from inside the dialog ---------------------------------

/// A prompt that answers a fixed sequence of dialogs, so a session that
/// reopens can be driven end to end.
enum Step {
    Pick(String),
    Select(Vec<usize>),
    SwitchBack,
}

struct Scripted {
    steps: RefCell<std::collections::VecDeque<Step>>,
    seen_first_items: RefCell<Vec<String>>,
    seen_tabs: RefCell<Vec<Vec<(String, bool)>>>,
}

impl Scripted {
    fn new(steps: Vec<Step>) -> Self {
        Self {
            steps: RefCell::new(steps.into()),
            seen_first_items: RefCell::new(Vec::new()),
            seen_tabs: RefCell::new(Vec::new()),
        }
    }
}

impl Scripted {
    fn note_tabs(&self, rows: &[Row]) {
        self.seen_tabs.borrow_mut().push(
            rows.iter()
                .filter(|r| r.marker.as_deref() == Some("tab"))
                .map(|r| (r.id.clone(), r.active))
                .collect(),
        );
    }
}

impl Prompt for Scripted {
    fn choose(&self, _: &[String], _: &str, _: &str) -> Option<String> {
        None
    }
    fn choose_many(&self, _: &[String], _: &str, _: &str) -> Option<Vec<String>> {
        None
    }
    fn choose_rows(&self, rows: &[Row], _t: &str, _p: &str) -> Option<String> {
        self.seen_first_items.borrow_mut().push(rows[0].id.clone());
        self.note_tabs(rows);
        match self.steps.borrow_mut().pop_front()? {
            Step::Pick(want) => rows
                .iter()
                .find(|r| r.id == want || r.label.ends_with(want.as_str()))
                .map(|r| r.id.clone()),
            _ => None,
        }
    }
    fn choose_many_rows(&self, rows: &[Row], _t: &str, _p: &str) -> Option<Vec<String>> {
        self.seen_first_items.borrow_mut().push(rows[0].id.clone());
        self.note_tabs(rows);
        let real = selectable(rows);
        match self.steps.borrow_mut().pop_front()? {
            Step::SwitchBack => Some(vec![TO_ONE.to_string()]),
            Step::Select(picked) => {
                Some(picked.iter().filter_map(|n| real.get(n - 1).map(|r| r.id.clone())).collect())
            }
            _ => None,
        }
    }
    fn ask_text(&self, _t: &str, _p: &str) -> Option<String> {
        None
    }
}

#[test]
fn every_mode_is_offered_as_a_tab_with_the_showing_one_marked() {
    let wm = FakeWm::new();
    let prompt = Scripted::new(vec![Step::Pick(TO_MANY.into()), Step::Select(vec![1])]);
    let _ = app::run_session(&wm, &prompt, &config(), &[], &[], &[]);

    // A mode's front door offers both tabs with one marked. Every step after
    // that carries none: switching mode halfway through picking windows, or
    // deciding where they go, is not something anyone means to do.
    let seen = prompt.seen_tabs.borrow().clone();
    let with_tabs: Vec<&Vec<(String, bool)>> = seen.iter().filter(|t| !t.is_empty()).collect();
    assert_eq!(with_tabs.len(), 2, "one tab strip per mode chooser");

    for tabs in &with_tabs {
        assert_eq!(tabs.len(), 3, "every mode should always be offered");
        assert_eq!(tabs.iter().filter(|(_, active)| *active).count(), 1);
    }
    assert!(with_tabs[0].iter().any(|(id, active)| id == TO_ONE && *active));
    assert!(with_tabs[1].iter().any(|(id, active)| id == TO_MANY && *active));
}

#[test]
fn switching_to_several_then_moving_them() {
    let wm = FakeWm::new();
    let prompt = Scripted::new(vec![
        Step::Pick(TO_MANY.into()),
        Step::Select(vec![1, 2]),
        Step::Pick("t-forms".into()),
    ]);
    match app::run_session(&wm, &prompt, &config(), &[], &[], &[]) {
        Session::Batch(b) => {
            assert_eq!(b.target.as_deref(), Some("t-forms"));
            assert_eq!(b.moved(), 2);
        }
        Session::Single(_) => panic!("expected a batch"),
    }
}

#[test]
fn switching_across_and_back_lands_on_the_focused_window() {
    let wm = FakeWm::new();
    let prompt = Scripted::new(vec![
        Step::Pick(TO_MANY.into()),
        Step::SwitchBack,
        Step::Pick("ac-app".into()),
    ]);
    match app::run_session(&wm, &prompt, &config(), &[], &[], &[]) {
        Session::Single(r) => {
            assert_eq!(r.outcome, Outcome::Moved { to: "ac-app".into(), created: false });
            // The focused window, not one chosen from the list.
            assert_eq!(r.window.unwrap().id, "11513");
        }
        Session::Batch(_) => panic!("expected a single move"),
    }
}

#[test]
fn endless_toggling_gives_up_rather_than_spinning() {
    /// Always picks whichever tab is not showing, which is the worst a person
    /// could do and must still terminate.
    struct AlwaysSwitch;
    impl Prompt for AlwaysSwitch {
        fn choose(&self, _i: &[String], _t: &str, _p: &str) -> Option<String> {
            None
        }
        fn choose_many(&self, _i: &[String], _t: &str, _p: &str) -> Option<Vec<String>> {
            None
        }
        fn choose_rows(&self, rows: &[Row], _t: &str, _p: &str) -> Option<String> {
            rows.iter()
                .find(|r| r.marker.as_deref() == Some("tab") && !r.active)
                .map(|r| r.id.clone())
        }
        fn choose_many_rows(&self, rows: &[Row], _t: &str, _p: &str) -> Option<Vec<String>> {
            self.choose_rows(rows, "", "").map(|id| vec![id])
        }
        fn ask_text(&self, _t: &str, _p: &str) -> Option<String> {
            None
        }
    }
    let wm = FakeWm::new();
    match app::run_session(&wm, &AlwaysSwitch, &config(), &[], &[], &[]) {
        Session::Single(r) => assert_eq!(r.outcome, Outcome::Cancelled),
        Session::Batch(_) => panic!("expected the loop to give up"),
    }
    assert!(wm.calls.borrow().is_empty(), "nothing should have moved");
}

// --- windows that come along to every task ---------------------------------

#[test]
fn picking_all_workspaces_tags_rather_than_moves() {
    let wm = FakeWm::new();
    let run = app::run(&wm, &FakePrompt::picking(ALL), &config(), &[], &[], &[]);

    assert_eq!(run.outcome, Outcome::Following);
    // A standing instruction, not a destination: the window stays put.
    assert!(wm.calls.borrow().is_empty());
}

#[test]
fn picking_it_again_takes_the_window_off_the_list() {
    let wm = FakeWm::new();
    let following = vec!["11513".to_string()];
    let run = app::run(&wm, &FakePrompt::picking(ALL), &config(), &[], &following, &[]);

    assert_eq!(run.outcome, Outcome::Unfollowing);
    assert!(wm.calls.borrow().is_empty());
}

#[test]
fn a_follower_is_brought_along() {
    let wm = FakeWm::new();
    let following = vec!["7695".to_string()];
    let run = app::run(&wm, &FakePrompt::picking("t-forms"), &config(), &[], &following, &[]);

    assert_eq!(run.outcome, Outcome::Moved { to: "t-forms".into(), created: true });
    assert_eq!(run.brought.len(), 1);
    assert_eq!(run.brought[0].0.id, "7695");

    // No focus anywhere, so nothing to hand back.
    assert_eq!(
        *wm.calls.borrow(),
        vec![
            Call::MoveById("11513".into(), "t-forms".into()),
            Call::MoveById("7695".into(), "t-forms".into()),
        ]
    );
}

#[test]
fn a_follower_comes_from_any_workspace() {
    // 19369 lives in t-mail, nowhere near the window being slung. Fetching it
    // used to mean focusing it, which restored all of t-mail first; naming it
    // costs nothing, so distance no longer matters.
    let wm = FakeWm::new();
    let following = vec!["19369".to_string()];
    let run = app::run(&wm, &FakePrompt::picking("t-forms"), &config(), &[], &following, &[]);

    assert_eq!(run.outcome, Outcome::Moved { to: "t-forms".into(), created: true });
    assert_eq!(run.brought.len(), 1);
    assert_eq!(run.brought[0].0.id, "19369");
}

#[test]
fn a_follower_already_in_the_target_is_not_moved() {
    let wm = FakeWm::new();
    let following = vec!["7695".to_string()];
    let run = app::run(&wm, &FakePrompt::picking("infra"), &config(), &[], &following, &[]);

    // Slinging to where the window already is moves nothing at all.
    assert_eq!(run.outcome, Outcome::SameWorkspace);
    assert!(run.brought.is_empty());
}

#[test]
fn an_unreachable_follower_does_not_break_the_sling() {
    let mut wm = FakeWm::new();
    wm.focus_refuses = vec!["7695".into()];
    let following = vec!["7695".to_string()];
    let run = app::run(&wm, &FakePrompt::picking("t-forms"), &config(), &[], &following, &[]);

    // The window the user asked about still moved.
    assert_eq!(run.outcome, Outcome::Moved { to: "t-forms".into(), created: true });
    assert_eq!(run.brought[0].1, Outcome::MoveFailed { to: "t-forms".into() });
}

#[test]
fn no_followers_means_no_extra_queries() {
    let wm = FakeWm::new();
    let run = app::run(&wm, &FakePrompt::picking("t-forms"), &config(), &[], &[], &[]);

    assert!(run.brought.is_empty());
    assert_eq!(*wm.calls.borrow(), vec![Call::MoveById("11513".into(), "t-forms".into())]);
}

#[test]
fn following_costs_no_workspace_restores() {
    use sling::app::restores_for;
    // It used to cost 1 + 2(n-1) restores, because moving a window dragged the
    // view along and stranded the rest. Naming the window costs none.
    for n in 0..5 {
        assert_eq!(restores_for(n), 0);
    }
}

#[test]
fn a_layout_matches_by_id_then_by_app_and_title() {
    use sling::store::{Layout, Placed};

    let saved = Layout {
        at: "2026-09-20T00:00:00Z".into(),
        windows: vec![
            Placed { id: "11513".into(), workspace: "t-mail".into(), app: "Brave".into(), title: "Mail".into() },
            // Its id changed — the application restarted since the snapshot.
            Placed { id: "9339".into(), workspace: "t-pair".into(), app: "WhatsApp".into(), title: "WhatsApp".into() },
            // Gone entirely.
            Placed { id: "404".into(), workspace: "infra".into(), app: "Dead".into(), title: "x".into() },
        ],
    };
    let live = vec![
        ("11513".to_string(), "Brave".to_string(), "Mail".to_string()),
        ("77777".to_string(), "WhatsApp".to_string(), "WhatsApp".to_string()),
    ];

    assert_eq!(
        saved.resolve(&live),
        vec![("11513", "t-mail"), ("77777", "t-pair")],
        "should match the moved id by app and title, and drop the window that is gone"
    );
}


#[test]
fn a_window_that_follows_is_marked_in_the_list() {
    // "all workspaces" is a standing instruction with nothing to show for it
    // in a list of workspaces — a follower is a window. The window list is
    // where it becomes visible.
    let wm = FakeWm::new();
    let following = vec!["9339".to_string(), "13029".to_string()];
    let menu = sling::picker::build_window_menu(&wm.all, "infra", &following);

    assert!(menu.rows.iter().all(|r| !r.pinned), "no follower is in this fixture yet");

    let with_one = vec!["7695".to_string()];
    let menu = sling::picker::build_window_menu(&wm.all, "infra", &with_one);
    let marked: Vec<&str> = menu.rows.iter().filter(|r| r.pinned).map(|r| r.id.as_str()).collect();
    assert_eq!(marked, vec!["7695"]);
}


#[test]
fn show_on_all_workspaces_is_a_row_that_carries_its_state() {
    // Not an action that announces itself: a setting, sitting with the current
    // workspace because both answer where this window lives.
    let wm = FakeWm::new();
    let off = app::run(&wm, &FakePrompt::picking("ac-app"), &config(), &[], &[], &[]);
    assert!(matches!(off.outcome, Outcome::Moved { .. }));

    // Rebuilt with the window following, the row reports it.
    let following = vec!["11513".to_string()];
    let prompt = FakePrompt::picking(ALL);
    let run = app::run(&wm, &prompt, &config(), &[], &following, &[]);
    assert_eq!(run.outcome, Outcome::Unfollowing, "picking it again turns it off");
}

/// Records the rows a flow offers, so what the list looks like can be asserted.
struct Capture(RefCell<Vec<Row>>);

#[test]
fn a_window_shown_everywhere_is_not_also_claimed_by_one_workspace() {
    // The two answers are mutually exclusive: a window on every workspace is
    // not in any of them, so only one of the pair may be ticked.
    let wm = FakeWm::new();
    let seen = Capture(RefCell::new(Vec::new()));
    let _ = app::run(&wm, &seen, &config(), &[], &["11513".to_string()], &[]);

    let rows = seen.0.borrow();
    let all = rows.iter().find(|r| r.id == ALL).expect("the setting should be offered");
    assert!(all.active, "showing everywhere should be ticked");
    assert!(
        !rows.iter().any(|r| r.marker.as_deref() == Some("here")),
        "no workspace should claim a window that is shown on all of them"
    );
}

impl Prompt for Capture {
    fn choose(&self, _: &[String], _: &str, _: &str) -> Option<String> {
        None
    }
    fn choose_many(&self, _: &[String], _: &str, _: &str) -> Option<Vec<String>> {
        None
    }
    fn choose_rows(&self, rows: &[Row], _: &str, _: &str) -> Option<String> {
        *self.0.borrow_mut() = rows.to_vec();
        None
    }
    fn ask_text(&self, _: &str, _: &str) -> Option<String> {
        None
    }
}

#[test]
fn the_here_group_opens_with_whichever_answer_is_true() {
    let wm = FakeWm::new();

    // Ordinarily the window is in a workspace, so that comes first.
    let seen = Capture(RefCell::new(Vec::new()));
    let _ = app::run(&wm, &seen, &config(), &[], &[], &[]);
    let here: Vec<String> = seen.0.borrow().iter()
        .filter(|r| r.section.as_deref() == Some("here"))
        .map(|r| r.id.clone())
        .collect();
    assert_eq!(here, vec!["infra".to_string(), ALL.to_string()]);

    // Shown everywhere, the setting is the answer and leads.
    let seen = Capture(RefCell::new(Vec::new()));
    let _ = app::run(&wm, &seen, &config(), &[], &["11513".to_string()], &[]);
    let here: Vec<String> = seen.0.borrow().iter()
        .filter(|r| r.section.as_deref() == Some("here"))
        .map(|r| r.id.clone())
        .collect();
    assert_eq!(here, vec![ALL.to_string(), "infra".to_string()]);
}

#[test]
fn a_setting_is_never_described_as_empty() {
    // "empty" is a thing a workspace can be. A setting has no windows to
    // count, so the front end is told not to look for one.
    let wm = FakeWm::new();
    let seen = Capture(RefCell::new(Vec::new()));
    let _ = app::run(&wm, &seen, &config(), &[], &[], &[]);

    let all = seen.0.borrow().iter().find(|r| r.id == ALL).cloned().expect("offered");
    assert_eq!(all.marker.as_deref(), Some("setting"));
    assert!(all.count.is_none());
}


#[test]
fn a_follower_on_another_screen_is_left_alone() {
    // Clicking a terminal on the second display must not drag the windows you
    // were working beside on the first one across to it.
    let mut wm = FakeWm::new();
    wm.all.push(window_on("42", "2", "tytoctl", "2"));

    let following = vec!["7695".to_string(), "42".to_string()];
    let run = app::run(&wm, &FakePrompt::picking("t-forms"), &config(), &[], &following, &[]);

    let brought: Vec<&str> = run.brought.iter().map(|(w, _)| w.id.as_str()).collect();
    assert_eq!(brought, vec!["7695"], "only the follower sharing this screen should move");
}


#[test]
fn jumping_goes_to_a_task_without_moving_anything() {
    let wm = FakeWm::new();
    let run = app::run_jump(&wm, &FakePrompt::picking("ac-app"), &config(), &[], &[]);

    assert_eq!(run.outcome, Outcome::Jumped { to: "ac-app".into() });
    // Nothing is slung: no window is named and none is moved.
    assert!(run.window.is_none());
    assert!(wm.calls.borrow().iter().all(|c| !matches!(c, Call::MoveById(..))));
}

#[test]
fn jumping_to_where_you_already_are_does_nothing() {
    let wm = FakeWm::new();
    let run = app::run_jump(&wm, &FakePrompt::picking("infra"), &config(), &[], &[]);
    assert_eq!(run.outcome, Outcome::SameWorkspace);
}

#[test]
fn a_key_can_open_straight_onto_the_jump_tab() {
    // The point of a second key: land on the list of tasks, not on the
    // question of where to send a window.
    let wm = FakeWm::new();
    let seen = Capture(RefCell::new(Vec::new()));
    let _ = app::run_session_from(
        app::Mode::Jump, &wm, &seen, &config(), &[], &[], &[],
    );
    let active: Vec<String> = seen.0.borrow().iter()
        .filter(|r| r.marker.as_deref() == Some("tab") && r.active)
        .map(|r| r.id.clone())
        .collect();
    assert_eq!(active, vec![sling::picker::TO_JUMP.to_string()]);
}
