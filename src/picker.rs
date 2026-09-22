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
pub const ONCE_LABEL: &str = "sling once";
pub const MANY_LABEL: &str = "sling many";
pub const JUMP_LABEL: &str = "jump to";

/// Not a workspace — a standing instruction. AeroSpace cannot put one window
/// in two places, so "everywhere" is emulated by bringing these along each
/// time something is slung.
pub const ALL: &str = "∞  all workspaces";
/// The same, for every window the application has.
pub const ALL_APP: &str = "__all_app__";

/// What the panel sends back when a row is pinned or unpinned rather than
/// chosen. The list has to be rebuilt afterwards, so it reopens.
pub const PIN: &str = "__pin__:";



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

    // Windows that share a label get their id shown, and only those: several
    // browser windows routinely carry the same title, and without this there
    // is no way to tell which row is which.
    let mut times_seen: BTreeMap<String, usize> = BTreeMap::new();
    for w in windows {
        *times_seen.entry(w.label()).or_default() += 1;
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
            let name = if w.title.is_empty() { w.app.clone() } else { w.title.clone() };
            let label = shorten(&name, 64);
            let shown = if times_seen.get(&w.label()).copied().unwrap_or(0) > 1 {
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
        }
    }

    /// A mode, drawn as a tab by a front end that can, and as an ordinary row
    /// by one that cannot.
    pub fn tab(id: &str, label: &str, active: bool) -> Self {
        Self { marker: Some("tab".into()), active, ..Self::action(id, label) }
    }

    /// What is being slung. Drawn in the header by a front end that can show
    /// an icon, and listed as a plain line by one that cannot.
    pub fn subject(app: &str, title: &str, bundle: &str) -> Self {
        Self {
            marker: Some("subject".into()),
            detail: (!title.is_empty()).then(|| shorten(title, 72)),
            bundle: Some(bundle.to_string()),
            ..Self::action("__subject__", app)
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
/// `counts` is `None` when the workspace query failed — then no counts are
/// shown at all, because a guessed number is worse than none.
pub fn build_menu(
    counts: Option<&BTreeMap<String, usize>>,
    known: &[String],
    cached: &[String],
    current: &str,
    order: &[String],
    labels: &BTreeMap<String, String>,
    pins: &[String],
) -> Menu {
    let occupied: Vec<String> = counts.map(|c| c.keys().cloned().collect()).unwrap_or_default();

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
            let label = match counts.and_then(|c| c.get(&name)) {
                _ if name == current => format!("{HERE}  {name}  (this window is here)"),
                Some(n) => format!("    {name}  ({n})"),
                None => format!("    {name}"),
            };
            back.insert(label.trim().to_string(), name.clone());
            items.push(label);
            rows.push(Row {
                id: name.clone(),
                label: name.clone(),
                section: Some(heading.clone()),
                count: counts.and_then(|c| c.get(&name)).copied(),
                marker: (name == current).then(|| "here".to_string()),
                pinned: pins.iter().any(|p| *p == name),
                detail: None,
                active: false,
                bundle: None,
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

    fn held(list: &[(&str, usize)]) -> BTreeMap<String, usize> {
        list.iter().map(|(w, n)| (w.to_string(), *n)).collect()
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
