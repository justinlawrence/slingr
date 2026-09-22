//! State that outlives a single sling: the name cache and the action log.
//!
//! Both live under `~/.local/state/sling/`, outside the repository — they are
//! observations, not source.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::config::home;

pub fn state_dir() -> PathBuf {
    home().join(".local/state/slingr")
}

/// Every workspace name ever seen or created.
///
/// AeroSpace forgets a workspace once it is empty and not on screen, so a task
/// invented through "new workspace…" would vanish from the menu as soon as it
/// emptied. The cache is what lets a name persist without editing config — and
/// what keeps the menu usable when AeroSpace is not answering.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct NameCache {
    #[serde(default)]
    pub workspaces: Vec<String>,
}

impl NameCache {
    fn path() -> PathBuf {
        state_dir().join("seen.json")
    }

    pub fn load() -> Self {
        fs::read_to_string(Self::path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Returns true if anything was new.
    pub fn merge(&mut self, names: &[String]) -> bool {
        let mut changed = false;
        for name in names {
            if !name.is_empty() && !self.workspaces.contains(name) {
                self.workspaces.push(name.clone());
                changed = true;
            }
        }
        if changed {
            self.workspaces.sort();
        }
        changed
    }

    pub fn save(&self) -> Result<()> {
        fs::create_dir_all(state_dir())?;
        fs::write(Self::path(), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

/// One JSON object per line, appended. The point is to find out whether the
/// taxonomy is right: which workspaces actually get used, how often a window
/// is re-slung, how often a new task is invented rather than filed.
pub struct ActionLog {
    path: PathBuf,
}

impl Default for ActionLog {
    fn default() -> Self {
        Self { path: state_dir().join("actions.jsonl") }
    }
}

impl ActionLog {
    /// A separate log for the watcher. Its lines are a timeline of what a
    /// herdr tab change did, which is a different question from where windows
    /// end up, and mixing the two makes both harder to read.
    pub fn watch() -> Self {
        Self { path: state_dir().join("watch.jsonl") }
    }
}

impl ActionLog {
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn append(&self, entry: &serde_json::Value) -> Result<()> {
        fs::create_dir_all(state_dir())?;
        let mut file = OpenOptions::new().create(true).append(true).open(&self.path)?;
        writeln!(file, "{}", serde_json::to_string(entry)?)?;
        Ok(())
    }
}

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// UTC timestamp, so a log line is readable without a tool.
pub fn iso8601(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's days-to-civil, the standard branch-free conversion.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_instants() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(1_000_000_000), "2001-09-09T01:46:40Z");
        // A leap day, which is where naive date maths goes wrong.
        assert_eq!(iso8601(1_709_164_800), "2024-02-29T00:00:00Z");
    }
}

/// Windows that get dragged along to whichever task you sling something to.
///
/// Poor man's persistence. AeroSpace has no sticky windows, so a window cannot
/// be in two workspaces; this keeps the handful that belong everywhere —
/// herdr, WhatsApp — with you by moving them each time you file something.
///
/// Window ids do not survive an application restart. That is accepted: the app
/// and title are stored alongside so a stale entry is legible, and re-tagging
/// is one pick.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct FollowList {
    #[serde(default)]
    pub windows: Vec<Follower>,
    /// Whole applications, by bundle id.
    ///
    /// Following a window is right for one of several — the herdr terminal
    /// among other terminals. Following an application is right when its
    /// windows are interchangeable and short-lived: Finder opens and closes
    /// windows all day, and "Finder belongs everywhere" is a fact about
    /// Finder, not about whichever window happens to be open.
    #[serde(default)]
    pub apps: Vec<FollowedApp>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FollowedApp {
    pub bundle: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Follower {
    pub id: String,
    #[serde(default)]
    pub app: String,
    #[serde(default)]
    pub title: String,
}

impl FollowList {
    fn path() -> PathBuf {
        state_dir().join("follow.json")
    }

    pub fn load() -> Self {
        fs::read_to_string(Self::path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn ids(&self) -> Vec<String> {
        self.windows.iter().map(|w| w.id.clone()).collect()
    }

    pub fn bundles(&self) -> Vec<String> {
        self.apps.iter().map(|a| a.bundle.clone()).collect()
    }

    pub fn follows_app(&self, bundle: &str) -> bool {
        !bundle.is_empty() && self.apps.iter().any(|a| a.bundle == bundle)
    }

    /// Returns true if the application is now followed.
    pub fn toggle_app(&mut self, bundle: &str, name: &str) -> bool {
        if let Some(at) = self.apps.iter().position(|a| a.bundle == bundle) {
            self.apps.remove(at);
            false
        } else {
            self.apps.push(FollowedApp { bundle: bundle.to_string(), name: name.to_string() });
            true
        }
    }

    pub fn contains(&self, id: &str) -> bool {
        self.windows.iter().any(|w| w.id == id)
    }

    /// Returns true if the window is now following.
    pub fn toggle(&mut self, id: &str, app: &str, title: &str) -> bool {
        if self.contains(id) {
            self.windows.retain(|w| w.id != id);
            false
        } else {
            self.windows.push(Follower {
                id: id.to_string(),
                app: app.to_string(),
                title: title.to_string(),
            });
            true
        }
    }

    pub fn save(&self) -> Result<()> {
        fs::create_dir_all(state_dir())?;
        fs::write(Self::path(), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

/// Where every window was, so a restart can be put back.
///
/// AeroSpace does not persist which workspace a window belongs to: restart it
/// and everything lands in whatever workspace each monitor happens to show.
/// This is the record that makes that recoverable.
///
/// Windows are keyed by id, which survives an AeroSpace restart because the
/// windows themselves are not recreated. It does not survive the *application*
/// restarting, so app and title are kept too — enough to recognise a window by
/// hand, and to match one whose id has changed.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Layout {
    #[serde(default)]
    pub at: String,
    #[serde(default)]
    pub windows: Vec<Placed>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Placed {
    pub id: String,
    pub workspace: String,
    #[serde(default)]
    pub app: String,
    #[serde(default)]
    pub title: String,
}

impl Layout {
    fn path() -> PathBuf {
        state_dir().join("layout.json")
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

    /// Match a saved placement to a live window: by id, else by app and title.
    pub fn resolve<'a>(&self, live: &'a [(String, String, String)]) -> Vec<(&'a str, &str)> {
        let mut out = Vec::new();
        for want in &self.windows {
            let hit = live
                .iter()
                .find(|(id, _, _)| *id == want.id)
                .or_else(|| {
                    if want.app.is_empty() && want.title.is_empty() {
                        None
                    } else {
                        live.iter().find(|(_, app, title)| {
                            *app == want.app && *title == want.title
                        })
                    }
                });
            if let Some((id, _, _)) = hit {
                out.push((id.as_str(), want.workspace.as_str()));
            }
        }
        out
    }
}

/// Tasks you want at the top.
///
/// A task is a herdr tab, and pinning is about the tab — not the project it
/// belongs to. Pinned tasks rise within their own folder rather than forming a
/// group of their own, so the grouping stays intact.
///
/// Separate from the follow list: that is about windows that belong
/// everywhere, this is about tasks worth reaching quickly.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Pins {
    #[serde(default, alias = "workspaces", alias = "folders")]
    pub tasks: Vec<String>,
}

impl Pins {
    fn path() -> PathBuf {
        state_dir().join("pins.json")
    }

    pub fn load() -> Self {
        fs::read_to_string(Self::path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Returns true if it is now pinned.
    pub fn toggle(&mut self, task: &str) -> bool {
        if let Some(at) = self.tasks.iter().position(|t| t == task) {
            self.tasks.remove(at);
            false
        } else {
            self.tasks.push(task.to_string());
            true
        }
    }

    pub fn save(&self) -> Result<()> {
        fs::create_dir_all(state_dir())?;
        fs::write(Self::path(), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

/// Where windows were deliberately put, read back out of the action log.
///
/// The log is cumulative and survives a reboot, which the snapshot does not:
/// window ids are all new after one, and the automatic snapshot overwrites the
/// good layout with the scattered one as soon as anything changes. What
/// survives is the app and title of every window ever slung, and where it was
/// sent — enough to put most of them back.
///
/// The last destination wins, so a window slung twice ends where it ended up.
pub fn placements_from_log() -> Vec<((String, String), String)> {
    let path = ActionLog::default().path().clone();
    fs::read_to_string(path).map(|text| placements_from(&text)).unwrap_or_default()
}

pub fn placements_from(text: &str) -> Vec<((String, String), String)> {
    let mut out: Vec<((String, String), String)> = Vec::new();
    for line in text.lines() {
        let Ok(row) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if row["outcome"].as_str() != Some("moved") {
            continue;
        }
        let (Some(to), Some(app), Some(title)) = (
            row["to"].as_str(),
            row["window"]["app"].as_str(),
            row["window"]["title"].as_str(),
        ) else {
            continue;
        };
        if app.is_empty() && title.is_empty() {
            continue;
        }
        let key = (app.to_string(), title.to_string());
        out.retain(|(k, _)| *k != key);
        out.push((key, to.to_string()));
    }
    out
}

#[cfg(test)]
mod placement_tests {
    use super::*;

    const LOG: &str = r#"
{"outcome":"moved","to":"ac-app","window":{"app":"Brave","title":"Framing"}}
{"outcome":"cancelled","window":{"app":"Brave","title":"Framing"}}
{"outcome":"moved","to":"t-pair","window":{"app":"Brave","title":"Framing"}}
{"outcome":"moved","to":"mail","window":{"app":"Mail","title":"Inbox"}}
{"outcome":"heading","to":"nowhere","window":{"app":"X","title":"Y"}}
not json
"#;

    #[test]
    fn the_last_deliberate_placement_wins() {
        let placed = placements_from(LOG);
        let found: Vec<(&str, &str)> =
            placed.iter().map(|((a, t), w)| (a.as_str(), w.as_str())).collect();
        assert!(found.contains(&("Brave", "t-pair")), "{found:?}");
        assert!(!found.iter().any(|(_, w)| *w == "ac-app"), "superseded: {found:?}");
        assert!(found.contains(&("Mail", "mail")));
    }

    #[test]
    fn only_moves_count_as_placements() {
        // A cancelled pick expresses no intent, and a heading names no task.
        let placed = placements_from(LOG);
        assert_eq!(placed.len(), 2, "{placed:?}");
    }

    #[test]
    fn survives_a_log_it_cannot_read() {
        assert!(placements_from("").is_empty());
        assert!(placements_from("garbage
{}
").is_empty());
    }
}
