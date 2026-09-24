//! Building the sling menu, and the naming rules for workspaces.
//!
//! Everything here is pure — no AeroSpace, no dialogs, no clock — so the part
//! with the fiddly rules in it stays testable. The impure edges live in
//! `aerospace` and `dialog`.

use std::collections::BTreeMap;

pub const NEW: &str = "＋  new workspace…";
/// The mode row. `choose from list` is a flat single-column control with no
/// tabs, so switching mode is a row you pick rather than a tab you click.
/// Mode ids. The labels live with the tabs that carry them.
pub const TO_MANY: &str = "__many__";
pub const TO_ONE: &str = "__once__";
pub const TO_JUMP: &str = "__jump__";
pub const TO_BOARD: &str = "__board__";
pub const ONCE_LABEL: &str = "sling once";
pub const MANY_LABEL: &str = "sling many";
pub const JUMP_LABEL: &str = "jump to";
pub const BOARD_LABEL: &str = "board";

/// Every mode id, in the order they are drawn.
///
/// One list, because the tab strip and the flows that read a chosen tab back
/// must agree about what a tab *is*. They did not once: the board was added to
/// the strip and not to the three `choice == TO_…` checks that recognise one,
/// so clicking it tried to sling the window to a workspace called
/// `__board__`. A mode is now a question asked of this list.
pub const MODES: [&str; 4] = [TO_ONE, TO_MANY, TO_JUMP, TO_BOARD];

/// Whether a chosen row names a mode rather than a destination.
pub fn is_mode(choice: &str) -> bool {
    MODES.contains(&choice)
}

/// What the board sends back when a window is dragged onto a task rather than
/// chosen: `__sling__:<window id>:<task>`.
///
/// Dragging is the only answer that names two things at once, and the panel
/// protocol is one string per line, so both travel in the one string. Neither
/// half can contain a `:` — window ids are numbers and
/// `sanitise_workspace` strips everything that is not a word character.
pub const SLING: &str = "__sling__:";

/// Read a dragged sling back into the window and where it was dropped.
pub fn sling_parts(choice: &str) -> Option<(&str, &str)> {
    let (window, workspace) = choice.strip_prefix(SLING)?.split_once(':')?;
    (!window.is_empty() && !workspace.is_empty()).then_some((window, workspace))
}

/// Not a workspace — a standing instruction. AeroSpace cannot put one window
/// in two places, so "everywhere" is emulated by bringing these along each
/// time something is slung.
pub const ALL: &str = "∞  all workspaces";
/// The same, for every window the application has.
pub const ALL_APP: &str = "__all_app__";

/// Applications that belong everywhere without being asked.
///
/// Finder is the archetype. It opens and closes windows all day, so a window
/// id does not survive it and following a particular window is useless; and
/// activating it from the Dock focuses whichever window it already has, which
/// drags you to that window's task. Being everywhere by nature removes the
/// reason focus leaves — see `docs/FINDINGS.md`.
///
/// Hard-coded on purpose, for now. The shape this wants to grow into is a
/// list in `workspaces.toml` that a person can add to; what it must not
/// become is a rule that guesses.
pub const GLOBAL_BY_NATURE: &[&str] = &["com.apple.finder"];

/// Whether an application is one of those, and so follows you without ever
/// having been added to the follow list.
pub fn is_global_by_nature(bundle: &str) -> bool {
    !bundle.is_empty() && GLOBAL_BY_NATURE.contains(&bundle)
}

/// What the panel sends back when a row is pinned or unpinned rather than
/// chosen. The list has to be rebuilt afterwards, so it reopens.
pub const PIN: &str = "__pin__:";

/// A task named in the search box rather than in a second dialog. The typed
/// text follows the prefix; it is sanitised here, not by whoever typed it.
pub const NEW_NAMED: &str = "__new__:";



pub fn all_row(following: bool) -> String {
    if following {
        format!("{ALL}  (following — pick to stop)")
    } else {
        format!("{ALL}  (bring along to every task)")
    }
}
pub const SEP: &str = "──────";
/// Marks the row for the workspace the window is in now. Deliberately the
/// only per-window marker in the list: a second symbol alongside it gets read
/// as saying something about the window too, which is how a tick meaning
/// "this workspace holds windows" came to look like "your window is here".
pub const HERE: &str = "●";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub id: String,
    pub workspace: String,
    /// Which screen it is on. A window that belongs everywhere belongs
    /// everywhere *on its own screen*; dragging it to another one takes it
    /// away from the work it was sitting beside.
    pub monitor: String,
    pub app: String,
    pub title: String,
    /// Bundle id, so a front end can show the application's icon instead of
    /// repeating its name on every row.
    pub bundle: String,
}

impl Window {
    /// How the window is described at the top of the dialog.
    pub fn label(&self) -> String {
        if self.title.is_empty() {
            self.app.clone()
        } else {
            format!("{} — {}", self.app, self.title)
        }
    }
}

/// Read one
/// `%{window-id}|%{workspace}|%{monitor-id}|%{app-name}|%{app-bundle-id}|%{window-title}`
/// row.
///
/// Titles contain `|` often enough to matter, so only the first five
/// separators are significant and the title keeps whatever it holds.
pub fn parse_window(line: &str) -> Option<Window> {
    if line.trim().is_empty() {
        return None;
    }
    let mut parts = line.splitn(6, '|');
    let id = parts.next()?.trim().to_string();
    if id.is_empty() {
        return None;
    }
    let workspace = parts.next()?.trim().to_string();
    Some(Window {
        id,
        workspace,
        monitor: parts.next().unwrap_or("").trim().to_string(),
        app: parts.next().unwrap_or("").trim().to_string(),
        bundle: parts.next().unwrap_or("").trim().to_string(),
        title: parts.next().unwrap_or("").trim().to_string(),
    })
}

/// How many icons stand in for a workspace on its row. Five is what still
/// fits beside the name at the largest font scale the panel offers.
pub const STACK_MAX: usize = 5;

/// What a workspace holds: how many windows, and which applications they
/// belong to, in the order their icons should be drawn.
///
/// The count alone used to be enough. It answers "how full", which is a
/// weaker question than "what kind of work is in here" — and the second is
/// the one you are actually asking when you scan the list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Occupancy {
    pub count: usize,
    /// Bundle ids, already ordered and capped. The front end draws them as
    /// given rather than deciding any of this for itself.
    pub stack: Vec<String>,
}

/// Group windows by workspace and work out each one's icon stack.
pub fn occupancy_of(windows: &[Window]) -> BTreeMap<String, Occupancy> {
    occupancy_from(windows.iter().map(|w| (w.workspace.as_str(), w.bundle.as_str())))
}

/// The same, from bare `(workspace, bundle)` pairs — which is all AeroSpace
/// has to be asked for, and a much cheaper query than the whole window list.
///
/// Rarest application first. Nearly every window on a working machine tends to
/// belong to the same browser, so a stack ordered by frequency would be five
/// identical icons on almost every row and would separate nothing. Leading
/// with the unusual application means the cap only ever hides *duplicates*
/// until a workspace holds more than `STACK_MAX` distinct applications — so a
/// task with an editor and a spreadsheet in it reads as one at a glance,
/// instead of as another row of browsers.
///
/// Multiplicity is kept rather than collapsed to one icon per application:
/// the length of the stack is itself a reading of how full the workspace is,
/// and that is free.
pub fn occupancy_from<'a>(pairs: impl Iterator<Item = (&'a str, &'a str)>) -> BTreeMap<String, Occupancy> {
    let mut per_workspace: BTreeMap<String, (usize, BTreeMap<String, usize>)> = BTreeMap::new();
    for (workspace, bundle) in pairs {
        if workspace.is_empty() {
            continue;
        }
        let entry = per_workspace.entry(workspace.to_string()).or_default();
        entry.0 += 1;
        if !bundle.is_empty() {
            *entry.1.entry(bundle.to_string()).or_default() += 1;
        }
    }

    per_workspace
        .into_iter()
        .map(|(workspace, (count, per_app))| {
            let mut apps: Vec<(String, usize)> = per_app.into_iter().collect();
            // Ties break on the bundle id, so the same workspace draws the
            // same stack every time the panel opens. A stack that reshuffles
            // between openings is a stack you have to read rather than
            // recognise, which is the whole point of having one.
            apps.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));

            let mut stack: Vec<String> = Vec::new();
            'fill: for (bundle, n) in apps {
                for _ in 0..n {
                    if stack.len() == STACK_MAX {
                        break 'fill;
                    }
                    stack.push(bundle.clone());
                }
            }
            (workspace, Occupancy { count, stack })
        })
        .collect()
}

/// The tab's own title, with the browser's furniture taken off the end.
///
/// Chromium writes `<tab title> - <Browser> – <profile>` into the window
/// title, so every browser row in the list ends with the same thirty
/// characters — which is precisely the part that distinguishes nothing. The
/// application's icon already says which browser it is, and the profile says
/// less than that.
///
/// Only removed when the name in the suffix is the application's own, so a
/// page genuinely called "Something - Else – Other" keeps its title.
pub fn tab_title(app: &str, title: &str) -> String {
    let trimmed = strip_memory_note(strip_browser_suffix(app, title)).trim();
    // Never strip a title down to nothing; a row with no words is worse than
    // one with the browser's name on it.
    if trimmed.is_empty() { title.trim().to_string() } else { trimmed.to_string() }
}

/// `… - Brave – justin@example.com` → `…`
///
/// The separator before the profile is an en dash and the one before the
/// browser is a hyphen. That asymmetry is Chromium's, and it is what makes
/// this safe to do by string: an em dash in a document title (`wp.dump —
/// wp.dump`) does not match.
fn strip_browser_suffix<'a>(app: &str, title: &'a str) -> &'a str {
    let Some((before_profile, _)) = title.rsplit_once(" \u{2013} ") else { return title };
    let Some((tab, browser)) = before_profile.rsplit_once(" - ") else { return title };
    if browser.is_empty() || !app.starts_with(browser) {
        return title;
    }
    tab
}

/// `My Tasks - High memory usage - 1.4 GB` → `My Tasks`
///
/// Chromium appends this to a heavy tab's title, and the figure climbs while
/// you work — so the same window reads differently every time the picker
/// opens, which is the one thing a title must not do.
fn strip_memory_note(title: &str) -> &str {
    match title.rsplit_once(" - High memory usage - ") {
        Some((before, size)) if size.ends_with('B') => before,
        _ => title,
    }
}

/// Coerce a typed name into something AeroSpace will accept.
///
/// A `/` in a workspace name hangs AeroSpace on a modal dialog it never shows,
/// which wedges the server. Tasks are named after herdr tabs (`t/forms`), so
/// that slash arrives by reflex — translate it rather than reject it.
pub fn sanitise_workspace(raw: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
            out.push(ch);
            prev_dash = ch == '-';
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Bucket workspace names by the prefix before their first `-`.
///
/// Configured prefixes come first, the rest alphabetically, and unprefixed
/// names last under "elsewhere".
pub fn group_workspaces(
    names: &[String],
    order: &[String],
    labels: &BTreeMap<String, String>,
) -> Vec<(String, Vec<String>)> {
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for name in names {
        let prefix = match name.split_once('-') {
            Some((p, _)) => p.to_string(),
            None => "_other".to_string(),
        };
        groups.entry(prefix).or_default().push(name.clone());
    }

    let mut ordered: Vec<String> = order.iter().filter(|p| groups.contains_key(*p)).cloned().collect();
    let mut rest: Vec<String> = groups
        .keys()
        .filter(|p| !ordered.contains(p) && *p != "_other")
        .cloned()
        .collect();
    rest.sort();
    ordered.extend(rest);
    if groups.contains_key("_other") {
        ordered.push("_other".to_string());
    }

    ordered
        .into_iter()
        .map(|prefix| {
            let heading = if prefix == "_other" {
                "elsewhere".to_string()
            } else {
                labels.get(&prefix).cloned().unwrap_or_else(|| prefix.clone())
            };
            let mut members = groups.remove(&prefix).unwrap_or_default();
            members.sort();
            (heading, members)
        })
        .collect()
}

/// One line per window, grouped under its workspace.
///
/// Rows are addressed by a leading number rather than by their text: two
/// browser windows routinely share a title, and a chosen line comes back
/// trimmed, so matching on the label is both ambiguous and fragile.
pub struct WindowMenu {
    pub items: Vec<String>,
    pub rows: Vec<Row>,
    by_index: BTreeMap<usize, Window>,
}

impl WindowMenu {
    /// The window a chosen line refers to. Headings carry no index.
    pub fn window_for(&self, choice: &str) -> Option<&Window> {
        let (number, _) = choice.trim().split_once('\u{b7}')?;
        self.by_index.get(&number.trim().parse::<usize>().ok()?)
    }

    pub fn windows_for(&self, choices: &[String]) -> Vec<Window> {
        choices.iter().filter_map(|c| self.window_for(c)).cloned().collect()
    }
}

/// Shorten a title to keep the dialog a sane width, on a character boundary.
pub fn shorten_label(text: &str, limit: usize) -> String {
    shorten(text, limit)
}

/// Shorten a title to keep the dialog a sane width, on a character boundary.
fn shorten(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let kept: String = text.chars().take(limit.saturating_sub(1)).collect();
    format!("{}\u{2026}", kept.trim_end())
}

/// Build the window list. `first` names the workspace to show at the top —
/// normally the one being drained. `follow` marks the windows that come along
/// to every task, so their standing instruction is visible in the list rather
/// than only on the row that sets it.
pub fn build_window_menu(
    windows: &[Window],
    first: &str,
    follows: impl Fn(&Window) -> bool,
) -> WindowMenu {
    let mut groups: BTreeMap<String, Vec<&Window>> = BTreeMap::new();
    for w in windows {
        groups.entry(w.workspace.clone()).or_default().push(w);
    }

    let mut order: Vec<String> = groups.keys().cloned().collect();
    order.sort_by_key(|w| (w != first, w.clone()));

    // What each row will actually say. Worked out before anything is drawn,
    // because two titles that differ only in the part being stripped — the
    // memory figure, say — become the same row once stripped.
    let shown_as = |w: &Window| {
        if w.title.is_empty() { w.app.clone() } else { tab_title(&w.app, &w.title) }
    };

    // Windows that share a label get their id shown, and only those: several
    // browser windows routinely carry the same title, and without this there
    // is no way to tell which row is which. Counted on the shown name rather
    // than the raw one, or the disambiguation goes missing exactly where it
    // is needed.
    let mut times_seen: BTreeMap<String, usize> = BTreeMap::new();
    for w in windows {
        *times_seen.entry(format!("{}|{}", w.app, shown_as(w))).or_default() += 1;
    }

    let mut items = Vec::new();
    let mut rows = Vec::new();
    let mut by_index = BTreeMap::new();
    let mut index = 0usize;
    for workspace in order {
        let mut members = groups.remove(&workspace).unwrap_or_default();
        members.sort_by_key(|w| (w.app.clone(), w.title.clone(), w.id.clone()));
        items.push(format!("{SEP}  {workspace}  ({}) {SEP}", members.len()));
        for w in members {
            index += 1;
            // The icon says which application it is, so the row only has to
            // say which window — browser titles are long enough already.
            let name = shown_as(w);
            let label = shorten(&name, 64);
            let key = format!("{}|{}", w.app, name);
            let shown = if times_seen.get(&key).copied().unwrap_or(0) > 1 {
                format!("{label}  [{}]", w.id)
            } else {
                label
            };
            items.push(format!("{index:>3} \u{b7} {} — {shown}", w.app));
            rows.push(Row {
                id: w.id.clone(),
                label: shown.clone(),
                section: Some(workspace.clone()),
                count: None,
                marker: None,
                pinned: follows(w),
                detail: None,
                active: false,
                bundle: Some(w.bundle.clone()),
                stack: Vec::new(),
            });
            by_index.insert(index, w.clone());
        }
    }
    WindowMenu { items, rows, by_index }
}

impl Menu {
    /// The workspace a chosen line names, if it names one. Headings and the
    /// "new workspace" entry deliberately name nothing.
    pub fn workspace_for(&self, choice: &str) -> Option<&String> {
        self.back.get(choice.trim())
    }
}

/// A menu line with its parts still separate, for a front end that can lay
/// them out itself. The rendered `items` stay for the AppleScript fallback,
/// which can only take flat strings.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Row {
    pub id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marker: Option<String>,
    pub pinned: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// For a tab row: whether it is the one currently showing.
    #[serde(default)]
    pub active: bool,
    /// Bundle id for the icon, on window rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle: Option<String>,
    /// Bundle ids standing in for what a workspace holds, on task rows.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub stack: Vec<String>,
}

impl Row {
    pub fn action(id: &str, label: &str) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            section: None,
            count: None,
            marker: Some("action".into()),
            pinned: false,
            detail: None,
            active: false,
            bundle: None,
            stack: Vec::new(),
        }
    }

    /// A mode, drawn as a tab by a front end that can, and as an ordinary row
    /// by one that cannot.
    pub fn tab(id: &str, label: &str, active: bool) -> Self {
        Self { marker: Some("tab".into()), active, ..Self::action(id, label) }
    }

    /// What is being slung: its icon and what the window actually says it is.
    ///
    /// The application's name used to lead the header, with the title trailing
    /// after it in grey. But the icon already says "Brave", twice over, and
    /// the thing being identified is the *window* — so the title leads and the
    /// name is gone. An untitled window falls back to the application, which
    /// is then the only thing there is to say.
    pub fn subject(app: &str, title: &str, bundle: &str) -> Self {
        let name = if title.is_empty() { app.to_string() } else { tab_title(app, title) };
        Self {
            marker: Some("subject".into()),
            bundle: Some(bundle.to_string()),
            ..Self::action("__subject__", &shorten(&name, 72))
        }
    }

    /// A task holding nothing.
    ///
    /// It has no window to list, but it is still somewhere a window can be
    /// dropped — and without it the board could only rearrange what is already
    /// spread out, never tidy into a task that is waiting empty.
    pub fn space(name: &str) -> Self {
        Self {
            marker: Some("space".into()),
            section: Some(name.to_string()),
            ..Self::action(name, name)
        }
    }

    /// A setting that reads as an ordinary row and carries a tick, rather than
    /// an action that announces itself. Its state is the point.
    pub fn toggle(id: &str, label: &str, on: bool) -> Self {
        Self {
            marker: Some("setting".into()),
            active: on,
            section: Some("here".into()),
            ..Self::action(id, label)
        }
    }
}

pub struct Menu {
    pub items: Vec<String>,
    pub rows: Vec<Row>,
    /// Which tasks are pinned, so the front end can mark them.
    pub pinned: Vec<String>,
    /// Chosen line back to a workspace name, keyed on the *trimmed* label.
    ///
    /// `osascript` strips whitespace from what it returns, so a label indented
    /// for alignment ("    t-pair") comes back without its indent and would
    /// match nothing. Look up with `trim()` — see `workspace_for`.
    pub back: BTreeMap<String, String>,
}

/// Build the dialog list.
///
/// `held` is `None` when the workspace query failed — then no counts and no
/// icons are shown at all, because a guessed number is worse than none and a
/// guessed stack is worse than a guessed number.
pub fn build_menu(
    held: Option<&BTreeMap<String, Occupancy>>,
    known: &[String],
    cached: &[String],
    current: &str,
    order: &[String],
    labels: &BTreeMap<String, String>,
    pins: &[String],
) -> Menu {
    let occupied: Vec<String> = held.map(|h| h.keys().cloned().collect()).unwrap_or_default();

    let mut names: Vec<String> = Vec::new();
    for name in occupied.iter().chain(known).chain(cached).chain(std::iter::once(&current.to_string())) {
        if !name.is_empty() && !names.contains(name) {
            names.push(name.clone());
        }
    }

    // Where you are comes first, always. After that the folders, pinned ones
    // ahead of the rest but otherwise in their configured order.
    let lifted: Vec<(String, Vec<String>)> = if !current.is_empty()
        && names.iter().any(|n| n == current)
    {
        vec![("here".to_string(), vec![current.to_string()])]
    } else {
        Vec::new()
    };
    let already: Vec<String> = lifted.iter().flat_map(|(_, m)| m.clone()).collect();

    let mut items = vec![NEW.to_string()];
    let mut rows = vec![Row::action(NEW, NEW)];
    let mut back = BTreeMap::new();
    let folders: Vec<(String, Vec<String>)> = group_workspaces(&names, order, labels)
        .into_iter()
        .map(|(heading, members)| {
            let mut members: Vec<String> =
                members.into_iter().filter(|n| !already.contains(n)).collect();
            // Pinned tasks rise inside their folder. Sorting is stable, so the
            // rest keep their alphabetical order.
            members.sort_by_key(|name| !pins.iter().any(|p| p == name));
            (heading, members)
        })
        .filter(|(_, m)| !m.is_empty())
        .collect();

    let grouped: Vec<(String, Vec<String>)> = lifted.into_iter().chain(folders).collect();

    for (heading, members) in grouped {
        items.push(format!("{SEP}  {heading}  {SEP}"));
        for name in members {
            // A count says what it is about — the workspace — and says more
            // than a tick did. An empty task simply has no number.
            let held_here = held.and_then(|h| h.get(&name));
            let label = match held_here {
                _ if name == current => format!("{HERE}  {name}  (this window is here)"),
                Some(o) => format!("    {name}  ({})", o.count),
                None => format!("    {name}"),
            };
            back.insert(label.trim().to_string(), name.clone());
            items.push(label);
            rows.push(Row {
                id: name.clone(),
                label: name.clone(),
                section: Some(heading.clone()),
                count: held_here.map(|o| o.count),
                marker: (name == current).then(|| "here".to_string()),
                pinned: pins.iter().any(|p| *p == name),
                detail: None,
                active: false,
                bundle: None,
                stack: held_here.map(|o| o.stack.clone()).unwrap_or_default(),
            });
        }
    }
    Menu { items, rows, back, pinned: pins.to_vec() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("t".to_string(), "tyto".to_string()),
            ("ac".to_string(), "art corner".to_string()),
        ])
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn held(list: &[(&str, usize)]) -> BTreeMap<String, Occupancy> {
        list.iter()
            .map(|(w, n)| (w.to_string(), Occupancy { count: *n, stack: Vec::new() }))
            .collect()
    }

    fn win(workspace: &str, app: &str, bundle: &str) -> Window {
        Window {
            id: format!("{workspace}-{app}"),
            workspace: workspace.into(),
            monitor: "1".into(),
            app: app.into(),
            bundle: bundle.into(),
            title: String::new(),
        }
    }

    #[test]
    fn keeps_pipes_that_belong_to_the_title() {
        let w = parse_window("11513|infra|1|Brave Browser|com.brave.Browser|Inbox | Mail - Brave")
            .unwrap();
        assert_eq!(w.id, "11513");
        assert_eq!(w.workspace, "infra");
        assert_eq!(w.monitor, "1");
        assert_eq!(w.app, "Brave Browser");
        assert_eq!(w.bundle, "com.brave.Browser");
        assert_eq!(w.title, "Inbox | Mail - Brave");
    }

    #[test]
    fn tolerates_a_window_with_no_title() {
        let w = parse_window("19354|infra|1|Brave Browser|com.brave.Browser|").unwrap();
        assert_eq!(w.title, "");
        assert_eq!(w.label(), "Brave Browser");
    }

    #[test]
    fn rejects_rows_that_are_not_windows() {
        assert!(parse_window("").is_none());
        assert!(parse_window("   ").is_none());
        assert!(parse_window("|infra|1|Brave|com.brave.Browser|x").is_none());
    }

    #[test]
    fn translates_the_herdr_spelling_of_a_task() {
        assert_eq!(sanitise_workspace("t/forms"), "t-forms");
        assert_eq!(sanitise_workspace("ac/new thing"), "ac-new-thing");
    }

    #[test]
    fn collapses_runs_and_trims_edges() {
        assert_eq!(sanitise_workspace("House Hunt 2026"), "House-Hunt-2026");
        assert_eq!(sanitise_workspace("  spaced  "), "spaced");
        assert_eq!(sanitise_workspace("weird$name!"), "weird-name");
        assert_eq!(sanitise_workspace("a // b"), "a-b");
        assert_eq!(sanitise_workspace("///"), "");
    }

    #[test]
    fn leaves_a_name_that_is_already_legal_alone() {
        assert_eq!(sanitise_workspace("t-gant-workload"), "t-gant-workload");
        assert_eq!(sanitise_workspace("me-watson"), "me-watson");
    }

    #[test]
    fn groups_configured_prefixes_first_then_the_rest() {
        let grouped = group_workspaces(
            &names(&["zz-late", "ac-app", "house", "t-forms", "t-pair"]),
            &names(&["t", "ac"]),
            &labels(),
        );
        let headings: Vec<&str> = grouped.iter().map(|(h, _)| h.as_str()).collect();
        assert_eq!(headings, vec!["tyto", "art corner", "zz", "elsewhere"]);
        assert_eq!(grouped[0].1, names(&["t-forms", "t-pair"]));
        assert_eq!(grouped[3].1, names(&["house"]));
    }

    const BRAVE: &str = "com.brave.Browser";
    const GHOSTTY: &str = "com.mitchellh.ghostty";
    const ZED: &str = "dev.zed.Zed";
    const OFFICE: &str = "org.libreoffice.script";

    #[test]
    fn a_browser_row_says_what_the_tab_says() {
        assert_eq!(
            tab_title(
                "Brave Browser",
                "easyJet | Flights & holidays ✈️ Book low-cost airline tickets - Brave – justinl@example.com"
            ),
            "easyJet | Flights & holidays ✈️ Book low-cost airline tickets"
        );
        assert_eq!(
            tab_title("Google Chrome", "My Tasks - Google Chrome – Justin (example.uk)"),
            "My Tasks"
        );
        // A hyphen inside the tab's own title survives; only the last one, the
        // one before the browser's name, is furniture.
        assert_eq!(
            tab_title("Brave Browser", "Shared-Christy-Justin - Dropbox - Brave – jl@example.com"),
            "Shared-Christy-Justin - Dropbox"
        );
    }

    #[test]
    fn the_memory_warning_chromium_bolts_on_is_not_part_of_the_title() {
        // The figure climbs while you work, so the same window reads
        // differently every time the picker opens.
        assert_eq!(
            tab_title("Brave Browser", "My Tasks - High memory usage - 1.4 GB - Brave – justinl@example.com"),
            "My Tasks"
        );
        assert_eq!(
            tab_title(
                "Brave Browser",
                "Inbox (586) - justin@example.uk - Example Mail - High memory usage - 800 MB - Brave – justin@example.uk"
            ),
            "Inbox (586) - justin@example.uk - Example Mail"
        );
    }

    #[test]
    fn a_title_that_is_not_a_browsers_is_left_exactly_alone() {
        // Zed uses an em dash, Chromium an en dash. That is the whole
        // difference, and it is what keeps this safe to do by string.
        assert_eq!(tab_title("Zed", "wp.dump — wp.dump"), "wp.dump — wp.dump");
        assert_eq!(tab_title("Ghostty", "MacBook-Pro.local: tyto"), "MacBook-Pro.local: tyto");
        assert_eq!(
            tab_title("LibreOffice", "Lawrence_Studios_2025_calendar_year.xlsx"),
            "Lawrence_Studios_2025_calendar_year.xlsx"
        );
        // The suffix names an application that is not this one, so it stays.
        assert_eq!(
            tab_title("Brave Browser", "Reading about Safari - Safari – someone"),
            "Reading about Safari - Safari – someone"
        );
    }

    #[test]
    fn the_subject_leads_with_the_window_not_the_application() {
        let row = Row::subject("Brave Browser", "Arty Vitals - Brave – justinl@example.com", BRAVE);
        assert_eq!(row.label, "Arty Vitals");
        assert_eq!(row.detail, None, "the application's name is the icon's job");

        // Nothing to say about the window but which application it is.
        let bare = Row::subject("‎WhatsApp", "", "net.whatsapp.WhatsApp");
        assert_eq!(bare.label, "‎WhatsApp");
    }

    #[test]
    fn two_windows_left_alike_by_stripping_still_show_their_ids() {
        // Both are "My Tasks" once the memory figure goes. Without counting on
        // the shown name the ids would be dropped exactly where they matter.
        let windows = vec![
            Window {
                id: "239".into(),
                workspace: "1".into(),
                monitor: "1".into(),
                app: "Brave Browser".into(),
                bundle: BRAVE.into(),
                title: "My Tasks - High memory usage - 1.4 GB - Brave – a@b.c".into(),
            },
            Window {
                id: "4126".into(),
                workspace: "1".into(),
                monitor: "1".into(),
                app: "Brave Browser".into(),
                bundle: BRAVE.into(),
                title: "My Tasks - High memory usage - 972 MB - Brave – a@b.c".into(),
            },
        ];
        let menu = build_window_menu(&windows, "1", |_| false);
        let labels: Vec<&str> = menu.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, vec!["My Tasks  [239]", "My Tasks  [4126]"]);
    }

    #[test]
    fn the_stack_leads_with_the_application_that_is_unusual_here() {
        // me-tax as it actually stands: two browser windows, a spreadsheet and
        // an editor. The browser is the least informative thing in it.
        let held = occupancy_of(&[
            win("me-tax", "Brave Browser", BRAVE),
            win("me-tax", "Brave Browser", BRAVE),
            win("me-tax", "LibreOffice", OFFICE),
            win("me-tax", "Zed", ZED),
        ]);
        let me_tax = &held["me-tax"];
        assert_eq!(me_tax.count, 4);
        assert_eq!(me_tax.stack, vec![ZED, OFFICE, BRAVE, BRAVE]);
    }

    #[test]
    fn the_cap_only_ever_eats_duplicates() {
        // Nine browser windows and one System Settings. The odd one must
        // survive the cap, or the row says nothing the count did not.
        let mut windows: Vec<Window> = (0..9).map(|_| win("1", "Brave Browser", BRAVE)).collect();
        windows.push(win("1", "System Settings", "com.apple.systempreferences"));
        let held = occupancy_of(&windows);
        let pile = &held["1"];
        assert_eq!(pile.count, 10);
        assert_eq!(pile.stack.len(), STACK_MAX);
        assert_eq!(pile.stack[0], "com.apple.systempreferences");
        assert!(pile.stack[1..].iter().all(|b| b == BRAVE));
    }

    #[test]
    fn a_workspace_holding_one_kind_of_thing_still_shows_its_weight() {
        let windows: Vec<Window> = (0..8).map(|_| win("t-taskedit", "Brave Browser", BRAVE)).collect();
        let held = occupancy_of(&windows);
        assert_eq!(held["t-taskedit"].stack, vec![BRAVE; STACK_MAX]);
    }

    #[test]
    fn the_same_workspace_draws_the_same_stack_every_time() {
        // Two applications with one window each: without a tie-break the order
        // would follow whatever the map handed back, and the row would
        // reshuffle between openings.
        let windows = vec![win("t-pair", "Ghostty", GHOSTTY), win("t-pair", "Zed", ZED)];
        let first = occupancy_of(&windows);
        let mut reversed = windows.clone();
        reversed.reverse();
        assert_eq!(first["t-pair"].stack, occupancy_of(&reversed)["t-pair"].stack);
        // Equal counts, so the tie-break decides: bundle id, ascending.
        assert_eq!(first["t-pair"].stack, vec![GHOSTTY, ZED]);
    }

    #[test]
    fn a_window_with_no_bundle_is_counted_but_not_drawn() {
        let held = occupancy_of(&[win("odd", "Something", ""), win("odd", "Ghostty", GHOSTTY)]);
        assert_eq!(held["odd"].count, 2);
        assert_eq!(held["odd"].stack, vec![GHOSTTY]);
    }

    #[test]
    fn a_task_row_carries_the_icons_of_what_is_in_it() {
        let held = occupancy_of(&[
            win("ac-app", "Brave Browser", BRAVE),
            win("me-tax", "Zed", ZED),
            win("me-tax", "Brave Browser", BRAVE),
            win("me-tax", "Brave Browser", BRAVE),
        ]);
        let menu = build_menu(Some(&held), &[], &[], "ac-app", &names(&["ac"]), &labels(), &[]);
        let me_tax = menu.rows.iter().find(|r| r.id == "me-tax").expect("me-tax listed");
        assert_eq!(me_tax.stack, vec![ZED, BRAVE, BRAVE]);
        assert_eq!(me_tax.count, Some(3));
        // A name AeroSpace has never held has nothing to draw, and must not
        // borrow anyone else's icons.
        let menu = build_menu(Some(&held), &names(&["ac-empty"]), &[], "ac-app", &[], &labels(), &[]);
        let empty = menu.rows.iter().find(|r| r.id == "ac-empty").expect("ac-empty listed");
        assert!(empty.stack.is_empty());
        assert_eq!(empty.count, None);
    }

    #[test]
    fn a_wedged_aerospace_draws_no_icons_at_all() {
        let menu = build_menu(None, &[], &names(&["ac-shopify"]), "infra", &[], &labels(), &[]);
        assert!(menu.rows.iter().all(|r| r.stack.is_empty()));
    }

    #[test]
    fn marks_where_the_window_is_and_what_already_has_windows() {
        let menu = build_menu(Some(&held(&[("infra", 40), ("ac-app", 2)])),
            &names(&["ac-shopify"]),
            &[],
            "infra",
            &names(&["ac"]),
            &labels(), &[]);
        let items = menu.items.join("\n");
        assert!(items.contains("●  infra  (this window is here)"), "{items}");
        // The count is about the workspace, and cannot be read as saying
        // anything about the window being moved.
        assert!(items.contains("    ac-app  (2)"), "{items}");
        // Known but empty: offered, with no number.
        assert!(items.contains("    ac-shopify"), "{items}");
        assert!(!items.contains("ac-shopify  ("), "{items}");
    }

    #[test]
    fn a_label_resolves_without_its_indent() {
        // osascript returns choices trimmed, so the indent that aligns an
        // unoccupied entry never survives the round trip.
        let menu = build_menu(Some(&held(&[("infra", 1)])), &names(&["t-pair"]), &[], "infra", &[], &labels(), &[]);
        let indented = menu.items.iter().find(|i| i.contains("t-pair")).unwrap();
        assert!(indented.starts_with("    "), "expected an indented entry");
        assert_eq!(menu.workspace_for(indented.trim()), Some(&"t-pair".to_string()));
        assert_eq!(menu.workspace_for(indented), Some(&"t-pair".to_string()));
    }

    #[test]
    fn where_you_are_comes_first_and_pinned_tasks_rise_in_their_folder() {
        let menu = build_menu(
            Some(&held(&[("infra", 23), ("t-mail", 2), ("t-pair", 1), ("ac-app", 3)])),
            &names(&["t-forms"]),
            &[],
            "infra",
            &names(&["t", "ac"]),
            &labels(),
            &names(&["t-pair"]),
        );
        let order: Vec<&str> = menu
            .rows
            .iter()
            .filter_map(|r| r.section.as_deref())
            .fold(Vec::new(), |mut seen, section| {
                if seen.last() != Some(&section) {
                    seen.push(section);
                }
                seen
            });
        assert_eq!(order.first(), Some(&"here"), "{order:?}");

        // Pinned task rises inside tyto without making a group of its own.
        let tyto: Vec<&str> = menu
            .rows
            .iter()
            .filter(|r| r.section.as_deref() == Some("tyto"))
            .map(|r| r.id.as_str())
            .collect();
        assert_eq!(tyto.first(), Some(&"t-pair"), "{tyto:?}");
        assert!(menu.rows.iter().find(|r| r.id == "t-pair").unwrap().pinned);
        assert_eq!(menu.rows.iter().filter(|r| r.id == "infra").count(), 1);
        assert_eq!(menu.pinned, names(&["t-pair"]));
    }

    #[test]
    fn unpinned_folders_keep_the_order_the_config_gave_them() {
        let menu = build_menu(
            Some(&held(&[("t-mail", 1), ("ac-app", 1), ("me-watson", 1)])),
            &[],
            &[],
            "",
            &names(&["t", "ac", "me"]),
            &labels(),
            &[],
        );
        let order: Vec<&str> = menu
            .rows
            .iter()
            .filter_map(|r| r.section.as_deref())
            .fold(Vec::new(), |mut seen, s| {
                if seen.last() != Some(&s) {
                    seen.push(s);
                }
                seen
            });
        assert_eq!(order, vec!["tyto", "art corner", "me"]);
    }

    #[test]
    fn the_current_workspace_is_not_also_listed_as_pinned() {
        // Pinning where you already are would put the same row in two places.
        let menu = build_menu(
            Some(&held(&[("infra", 2)])),
            &[],
            &[],
            "infra",
            &[],
            &labels(),
            &names(&["infra"]),
        );
        assert_eq!(menu.rows.iter().filter(|r| r.id == "infra").count(), 1);
        assert_eq!(menu.rows.iter().find(|r| r.id == "infra").unwrap().section.as_deref(), Some("here"));
    }

    #[test]
    fn headings_name_no_workspace() {
        let menu = build_menu(Some(&held(&[("infra", 1)])), &[], &[], "infra", &[], &labels(), &[]);
        for item in &menu.items {
            if item.starts_with(SEP) {
                assert!(menu.workspace_for(item).is_none(), "{item} resolved to a workspace");
            }
        }
        assert!(menu.workspace_for(NEW).is_none());
    }

    #[test]
    fn remembered_names_appear_when_aerospace_is_silent() {
        let menu = build_menu(None, &[], &names(&["ac-shopify"]), "infra", &[], &labels(), &[]);
        let items = menu.items.join("\n");
        assert!(items.contains("ac-shopify"));
        // No counts at all: a guessed number is worse than none. ("this window
        // is here" is parenthesised too, so look for a bracketed number.)
        let numbered = menu.items.iter().any(|i| {
            i.rsplit_once('(')
                .and_then(|(_, tail)| tail.strip_suffix(')'))
                .is_some_and(|n| n.parse::<usize>().is_ok())
        });
        assert!(!numbered, "showed a count without knowing one: {items}");
    }

    #[test]
    fn the_current_workspace_is_always_offered() {
        // It may be a workspace nothing else knows about.
        let menu = build_menu(Some(&held(&[])), &[], &[], "scratch", &[], &labels(), &[]);
        assert!(menu.back.values().any(|v| v == "scratch"));
    }
}

/// Render a row the way the flat AppleScript list needs it.
pub fn render_row(row: &Row) -> String {
    if row.marker.as_deref() == Some("action") {
        return row.label.clone();
    }
    let mut line = String::new();
    line.push_str(if row.marker.as_deref() == Some("here") { "\u{25cf}  " } else { "    " });
    line.push_str(&row.label);
    if let Some(n) = row.count.filter(|n| *n > 0) {
        line.push_str(&format!("  ({n})"));
    }
    if row.marker.as_deref() == Some("here") {
        line.push_str("  (this window is here)");
    }
    line
}

/// Which row a chosen line names. Matches on the rendered form, trimmed,
/// because a dialog hands its answer back without leading whitespace.
pub fn row_id_for(rows: &[Row], chosen: &str) -> Option<String> {
    let want = chosen.trim();
    rows.iter()
        .find(|r| render_row(r).trim() == want || r.id == want || r.label.trim() == want)
        .map(|r| r.id.clone())
}
