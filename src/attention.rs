//! Agents that want you, said in notifications that take you to them.
//!
//! herdr says so itself, but on macOS a click on its notification only brings
//! the terminal forward: the pane goes along only on Windows (see
//! docs/FINDINGS.md). Its plugin hooks do hear `pane.agent_status_changed`,
//! pane and all, so slingr says it instead, through terminal-notifier, and a
//! click runs `slingr answer`. That brings the agent's pane to the front,
//! herdr fires `tab.focused`, and `goto` follows it to the task as for any
//! other tab change. herdr's own popups are turned off (`[ui.toast] delivery
//! = "off"`), or every one would come twice.
//!
//! herdr's rules are kept. It waits a second, so a state that flickers says
//! nothing, and it stays quiet about the tab in front of you. A notification
//! is taken down once its agent has moved on, so Notification Centre holds
//! only what is still waiting.

use std::any::Any;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::herdr::{self, TabRef};
use crate::store::state_dir;

/// herdr's own default for `[ui.toast] delay_seconds`.
pub const DELAY: Duration = Duration::from_secs(1);

/// The subcommand a click runs. Named once, because it is written into a
/// shell command that no compiler reads.
pub const ANSWER: &str = "answer";

/// What there is to say about an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Stopped at a question or an approval: herdr's `blocked`.
    NeedsInput,
    /// Finished with nobody watching: herdr's `done`, which is idle but not
    /// yet seen. herdr marks a completion seen at once when its tab is in
    /// front and the terminal focused, so `done` already means herdr would
    /// have said so.
    Finished,
}

impl Kind {
    pub fn of(status: &str) -> Option<Kind> {
        match status {
            "blocked" => Some(Kind::NeedsInput),
            "done" => Some(Kind::Finished),
            _ => None,
        }
    }
}

/// An agent's status changing: which pane, and what it became.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub pane: String,
    pub status: String,
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    event: String,
    data: EventData,
}

#[derive(Deserialize)]
struct EventData {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    pane_id: String,
    #[serde(default)]
    agent_status: String,
}

/// The change a plugin hook is handed in `HERDR_PLUGIN_EVENT_JSON`.
///
/// A hook is handed `"event":"pane_agent_status_changed"`, with a `type` in
/// `data`; herdr's socket sends the same event as
/// `"event":"pane.agent_status_changed"`, with none (docs/FINDINGS.md). Both
/// are taken, so that herdr settling on either one costs nothing here.
pub fn parse_change(json: &str) -> Option<Change> {
    let envelope: Envelope = serde_json::from_str(json).ok()?;
    let named = |name: &str| {
        matches!(name, "pane.agent_status_changed" | "pane_agent_status_changed")
    };
    if !(named(&envelope.event) || named(&envelope.data.kind)) || envelope.data.pane_id.is_empty()
    {
        return None;
    }
    Some(Change { pane: envelope.data.pane_id, status: envelope.data.agent_status })
}

/// An agent, as `herdr agent get` describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub pane: String,
    pub tab: String,
    /// Which agent it is: `claude`, `codex`.
    pub name: String,
    pub status: String,
    /// herdr's count of state changes, as of this agent's latest. It is what
    /// tells a new stop from the same one heard twice.
    pub seq: u64,
    /// The title the agent gave its terminal, for a tab with no name.
    pub title: String,
}

#[derive(Deserialize)]
struct AgentReply {
    result: AgentResult,
}

#[derive(Deserialize)]
struct AgentResult {
    agent: AgentInfo,
}

#[derive(Deserialize)]
struct AgentInfo {
    #[serde(default)]
    pane_id: String,
    #[serde(default)]
    tab_id: String,
    #[serde(default)]
    agent: String,
    #[serde(default)]
    agent_status: String,
    #[serde(default)]
    state_change_seq: u64,
    #[serde(default)]
    terminal_title_stripped: String,
}

pub fn parse_agent(json: &str) -> Option<Agent> {
    let info = serde_json::from_str::<AgentReply>(json).ok()?.result.agent;
    if info.pane_id.is_empty() {
        return None;
    }
    Some(Agent {
        pane: info.pane_id,
        tab: info.tab_id,
        name: info.agent,
        status: info.agent_status,
        seq: info.state_change_seq,
        title: info.terminal_title_stripped,
    })
}

/// A notification that is up: what it said, and at which state change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Said {
    pub kind: Kind,
    pub seq: u64,
}

/// What is up in Notification Centre, by pane. Kept so that a status change
/// can take its notification down without asking terminal-notifier about
/// every agent that merely started work.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Posted {
    pub panes: BTreeMap<String, Said>,
}

impl Posted {
    fn path() -> PathBuf {
        state_dir().join("attention.json")
    }

    pub fn load() -> Self {
        fs::read_to_string(Self::path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Written whole and renamed into place: a hook killed halfway through
    /// would otherwise leave a file that reads as nothing up at all, and the
    /// notifications in it could never be taken down.
    pub fn save(&self) -> std::io::Result<()> {
        fs::create_dir_all(state_dir())?;
        let partial = Self::path().with_extension("json.partial");
        fs::write(&partial, serde_json::to_string_pretty(self)?)?;
        fs::rename(partial, Self::path())
    }
}

/// What to do about a pane's notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Post(Kind),
    Withdraw,
    /// Nothing, and why, for the log.
    Leave(&'static str),
}

/// The rule, with herdr and the screen taken out.
///
/// `now` is the agent as read after the wait, `None` if the pane has none any
/// more. `said` is what is already up for the pane. `looking` is herdr's
/// reason to keep quiet: the agent's tab is in front, and so is the terminal.
pub fn decide(now: Option<&Agent>, said: Option<&Said>, looking: bool) -> Act {
    match now.and_then(|agent| Some((agent, Kind::of(&agent.status)?))) {
        Some((agent, kind)) => {
            if said.is_some_and(|s| s.kind == kind && s.seq == agent.seq) {
                Act::Leave("already said")
            } else if looking {
                // Anything still up from an earlier stop no longer holds.
                if said.is_some() {
                    Act::Withdraw
                } else {
                    Act::Leave("in front of you")
                }
            } else {
                Act::Post(kind)
            }
        }
        None if said.is_some() => Act::Withdraw,
        None if now.is_none() => Act::Leave("no agent"),
        None => Act::Leave("moved on"),
    }
}

/// What a notification says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub title: String,
    pub message: String,
}

/// Titled with the task, as herdr's tab names it, since that is what says
/// where you are going. A tab with only herdr's number has no name, so the
/// title the agent gave its terminal stands in, then the pane.
pub fn note_for(kind: Kind, agent: &Agent, label: Option<&str>) -> Note {
    let named = |text: &&str| {
        let text = text.trim();
        !text.is_empty() && !text.chars().all(|c| c.is_ascii_digit())
    };
    let title = label
        .filter(named)
        .or(Some(agent.title.as_str()).filter(named))
        .unwrap_or(&agent.pane)
        .to_string();
    let who = if agent.name.is_empty() { "the agent" } else { agent.name.as_str() };
    let message = match kind {
        Kind::NeedsInput => format!("{who} needs input"),
        Kind::Finished => format!("{who} has finished"),
    };
    Note { title, message }
}

/// The group a pane's notification goes out under. A newer one replaces it,
/// and taking it down needs it.
pub fn group(pane: &str) -> String {
    format!("slingr.{pane}")
}

/// Single quotes, with any inside closed, escaped and reopened.
pub fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// The shell command a click runs. terminal-notifier hands it to
/// `/bin/sh -c`, so every part is quoted.
///
/// herdr's socket goes along when herdr gave one. The click runs from
/// terminal-notifier, long after the hook and with none of herdr's
/// environment, and a herdr session other than the default keeps its socket
/// elsewhere.
pub fn click_command(slingr: &Path, agent: &Agent, socket: Option<&str>) -> String {
    let mut command = String::new();
    if let Some(socket) = socket {
        command.push_str(&format!("HERDR_SOCKET_PATH={} ", sh_quote(socket)));
    }
    command.push_str(&format!(
        "{} {ANSWER} {}",
        sh_quote(&slingr.to_string_lossy()),
        sh_quote(&agent.pane)
    ));
    if !agent.tab.is_empty() {
        command.push_str(&format!(" --tab {}", sh_quote(&agent.tab)));
    }
    command
}

/// terminal-notifier's arguments for a notification that runs `click` when
/// clicked. terminal-notifier brings `terminal` forward first, then runs it.
pub fn post_args(note: &Note, group: &str, click: &str, terminal: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> =
        ["-title", &note.title, "-message", &note.message, "-group", group, "-execute", click]
            .iter()
            .map(|s| s.to_string())
            .collect();
    if let Some(bundle) = terminal {
        args.push("-activate".into());
        args.push(bundle.into());
    }
    args
}

/// The app herdr runs in, worked out as herdr works it out, so that a click
/// can bring it forward. A hook inherits the environment herdr was started
/// in, which is the terminal's shell; failing a terminal herdr knows, the app
/// that shell was started from.
pub fn terminal_bundle(term_program: Option<&str>, started_from: Option<&str>) -> Option<String> {
    let known = match term_program {
        Some("ghostty") => Some("com.mitchellh.ghostty"),
        Some("iTerm.app") => Some("com.googlecode.iterm2"),
        Some("WezTerm") => Some("com.github.wez.wezterm"),
        Some("Apple_Terminal") => Some("com.apple.Terminal"),
        _ => None,
    };
    known
        .map(str::to_string)
        .or_else(|| started_from.filter(|b| !b.is_empty()).map(str::to_string))
}

/// The bundle id in what `lsappinfo info -only bundleid` prints:
/// `"CFBundleIdentifier"="com.mitchellh.ghostty"`.
pub fn parse_bundle_id(text: &str) -> Option<String> {
    let (_, value) = text.trim().split_once('=')?;
    let value = value.trim().trim_matches('"');
    (!value.is_empty() && !value.contains("NULL")).then(|| value.to_string())
}

/// Everything the hook and the click touch outside themselves, so the order
/// they touch it in can be tested with no herdr and no Notification Centre.
pub trait World {
    /// Let a second pass, as herdr does.
    fn wait(&self);
    /// Hold the one lock every hook takes, until what this returns is
    /// dropped. `None` if it could not be had in time.
    fn hold(&self) -> Option<Box<dyn Any>>;
    /// What is up for a pane.
    fn said(&self, pane: &str) -> Option<Said>;
    /// Record what is up for a pane, or that nothing is.
    fn remember(&self, pane: &str, said: Option<Said>);
    /// The agent in a pane now, if there still is one.
    fn agent(&self, pane: &str) -> Option<Agent>;
    fn tab(&self, id: &str) -> Option<TabRef>;
    /// Whether the terminal herdr runs in is the app in front.
    fn terminal_in_front(&self) -> bool;
    /// Put a notification up for an agent. False if it could not go up.
    fn post(&self, note: &Note, agent: &Agent) -> bool;
    /// Take a pane's notification down.
    fn withdraw(&self, pane: &str);
    /// Bring an agent's pane to the front. False if there is no agent there.
    fn focus_agent(&self, pane: &str) -> bool;
    fn focus_tab(&self, id: &str) -> bool;
}

/// What a status change came to, for the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Report {
    /// Nothing was up and there is nothing to say: an agent starting or
    /// stopping work, which is nearly every change.
    Quiet,
    Posted { kind: Kind, title: String },
    /// terminal-notifier refused, and so did the fallback.
    NotPosted,
    Withdrawn,
    Left(&'static str),
    /// Another hook held the lock for too long.
    Busy,
}

/// What one status change does: the hook's whole job.
pub fn on_change(world: &dyn World, change: &Change) -> Report {
    let sayable = Kind::of(&change.status).is_some();
    // herdr's own wait, so that a state that flickers says nothing. Taken
    // before the lock, or every waiting hook would hold up all the others.
    if sayable {
        world.wait();
    }
    let Some(_held) = world.hold() else {
        return Report::Busy;
    };
    let said = world.said(&change.pane);
    if !sayable {
        if said.is_none() {
            return Report::Quiet;
        }
        // The change is news enough: whatever was said no longer holds.
        world.withdraw(&change.pane);
        world.remember(&change.pane, None);
        return Report::Withdrawn;
    }
    // Read again rather than taken from the event: what matters is whether
    // it still holds after the wait.
    let now = world.agent(&change.pane);
    let tab = now
        .as_ref()
        .filter(|agent| Kind::of(&agent.status).is_some())
        .and_then(|agent| world.tab(&agent.tab));
    let looking = tab.as_ref().is_some_and(|t| t.focused) && world.terminal_in_front();
    match decide(now.as_ref(), said.as_ref(), looking) {
        Act::Post(kind) => {
            let Some(agent) = now else { return Report::Left("no agent") };
            let note = note_for(kind, &agent, tab.as_ref().map(|t| t.label.as_str()));
            if !world.post(&note, &agent) {
                return Report::NotPosted;
            }
            world.remember(&agent.pane, Some(Said { kind, seq: agent.seq }));
            Report::Posted { kind, title: note.title }
        }
        Act::Withdraw => {
            world.withdraw(&change.pane);
            world.remember(&change.pane, None);
            Report::Withdrawn
        }
        Act::Leave(why) => Report::Left(why),
    }
}

/// Where a click took you.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Went {
    Agent,
    /// The agent had gone, so its tab instead.
    Tab,
    Nowhere,
}

/// What a click does: bring the agent's pane to the front, or its tab if the
/// agent has gone since. herdr then fires `tab.focused`, and `goto` takes you
/// to the task from there, as it would for a tab you clicked in herdr.
pub fn answer(world: &dyn World, pane: &str, tab: Option<&str>) -> Went {
    if world.focus_agent(pane) {
        return Went::Agent;
    }
    match tab {
        Some(tab) if world.focus_tab(tab) => Went::Tab,
        _ => Went::Nowhere,
    }
}

/// The real thing: herdr, terminal-notifier and the screen.
pub struct Live {
    /// The terminal herdr runs in, brought forward by a click.
    pub terminal: Option<String>,
    /// This binary, for a click to run.
    pub slingr: PathBuf,
    /// herdr's socket, when herdr gave one.
    pub socket: Option<String>,
}

impl Live {
    /// Read from the environment herdr hands a hook.
    pub fn from_env() -> Self {
        let var = |key: &str| std::env::var(key).ok().filter(|v| !v.is_empty());
        Self {
            terminal: terminal_bundle(
                var("TERM_PROGRAM").as_deref(),
                var("__CFBundleIdentifier").as_deref(),
            ),
            slingr: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("slingr")),
            socket: var("HERDR_SOCKET_PATH"),
        }
    }
}

/// Absolute where we can find it: a click runs from terminal-notifier, under
/// launchd's bare PATH.
fn notifier() -> String {
    for candidate in ["/opt/homebrew/bin/terminal-notifier", "/usr/local/bin/terminal-notifier"] {
        if Path::new(candidate).exists() {
            return candidate.to_string();
        }
    }
    "terminal-notifier".to_string()
}

/// Run something that should be quick, and give up on it if it is not.
fn run(program: &str, args: &[String]) -> Option<String> {
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(out)) if out.status.success() => {
            Some(String::from_utf8_lossy(&out.stdout).into_owned())
        }
        _ => None,
    }
}

/// The app in front. `lsappinfo` needs no permission, which matters: a hook
/// runs as herdr, and herdr has none.
fn frontmost() -> Option<String> {
    let front = run("/usr/bin/lsappinfo", &["front".to_string()])?;
    let asn = front.trim();
    if asn.is_empty() {
        return None;
    }
    let info = run(
        "/usr/bin/lsappinfo",
        &["info".to_string(), "-only".to_string(), "bundleid".to_string(), asn.to_string()],
    )?;
    parse_bundle_id(&info)
}

impl World for Live {
    fn wait(&self) {
        thread::sleep(DELAY);
    }

    /// An advisory lock, which the system lets go of when its process ends.
    /// Unlike `goto`'s, its file can stay behind: there is no recorded pid to
    /// judge, so nothing a crash leaves can hold it for ever.
    fn hold(&self) -> Option<Box<dyn Any>> {
        fs::create_dir_all(state_dir()).ok()?;
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(state_dir().join("attention.lock"))
            .ok()?;
        // Long enough for a queue of hooks that each post one notification.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match file.try_lock() {
                Ok(()) => return Some(Box::new(file)),
                Err(fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(_) => return None,
            }
        }
    }

    fn said(&self, pane: &str) -> Option<Said> {
        Posted::load().panes.get(pane).copied()
    }

    fn remember(&self, pane: &str, said: Option<Said>) {
        let mut posted = Posted::load();
        match said {
            Some(said) => posted.panes.insert(pane.to_string(), said),
            None => posted.panes.remove(pane),
        };
        let _ = posted.save();
    }

    fn agent(&self, pane: &str) -> Option<Agent> {
        parse_agent(&herdr::ask(&["agent", "get", pane])?)
    }

    fn tab(&self, id: &str) -> Option<TabRef> {
        herdr::tab_by_id(id)
    }

    fn terminal_in_front(&self) -> bool {
        self.terminal.as_ref().is_some_and(|terminal| frontmost().as_ref() == Some(terminal))
    }

    fn post(&self, note: &Note, agent: &Agent) -> bool {
        let click = click_command(&self.slingr, agent, self.socket.as_deref());
        let args = post_args(note, &group(&agent.pane), &click, self.terminal.as_deref());
        if run(&notifier(), &args).is_some() {
            return true;
        }
        // Said without the click rather than not said at all, since herdr's
        // own notifications are off.
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            crate::dialog::applescript_string(&note.message),
            crate::dialog::applescript_string(&note.title)
        );
        run("/usr/bin/osascript", &["-e".to_string(), script]).is_some()
    }

    fn withdraw(&self, pane: &str) {
        let _ = run(&notifier(), &["-remove".to_string(), group(pane)]);
    }

    fn focus_agent(&self, pane: &str) -> bool {
        herdr::focus_agent(pane)
    }

    fn focus_tab(&self, id: &str) -> bool {
        herdr::focus_tab(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What herdr handed the hook for a real finish on 12 November.
    const FROM_A_HOOK: &str = r#"{"data":{"agent":"claude","agent_status":"done","pane_id":"w7:p2H","type":"pane_agent_status_changed","workspace_id":"w7"},"event":"pane_agent_status_changed"}"#;

    /// The same event, as herdr's socket sent it that day.
    const FROM_THE_SOCKET: &str = r#"{"data":{"agent":"claude","agent_status":"done","pane_id":"w7:p2J","workspace_id":"w7"},"event":"pane.agent_status_changed"}"#;

    /// `herdr agent get w6:p5`, trimmed of what is not read.
    const AGENT_GET: &str = r#"{"id":"cli:agent:get","result":{"agent":{"agent":"claude","agent_status":"working","cwd":"/Users/justin/Dev/slingr","focused":false,"pane_id":"w6:p5","revision":5,"state_change_seq":5418,"tab_id":"w6:t5","terminal_title":"◐ me/slingr","terminal_title_stripped":"me/slingr","workspace_id":"w6"},"type":"agent_info"}}"#;

    fn agent(status: &str, seq: u64) -> Agent {
        Agent {
            pane: "w7:p2J".into(),
            tab: "w7:t2J".into(),
            name: "claude".into(),
            status: status.into(),
            seq,
            title: "CORS headers".into(),
        }
    }

    #[test]
    fn reads_an_agent_status_change_as_a_hook_is_handed_it() {
        assert_eq!(
            parse_change(FROM_A_HOOK),
            Some(Change { pane: "w7:p2H".into(), status: "done".into() })
        );
    }

    #[test]
    fn reads_the_spelling_herdrs_socket_uses_too() {
        assert_eq!(
            parse_change(FROM_THE_SOCKET),
            Some(Change { pane: "w7:p2J".into(), status: "done".into() })
        );
    }

    #[test]
    fn ignores_any_other_event_and_anything_unreadable() {
        let focus = r#"{"data":{"tab_id":"w7:tA","type":"tab_focused","workspace_id":"w7"},"event":"tab_focused"}"#;
        assert!(parse_change(focus).is_none());
        assert!(parse_change("not json").is_none());
        assert!(parse_change(r#"{"event":"pane.agent_status_changed","data":{}}"#).is_none());
    }

    #[test]
    fn reads_an_agent_as_herdr_describes_it() {
        let read = parse_agent(AGENT_GET).expect("one agent");
        assert_eq!(read.pane, "w6:p5");
        assert_eq!(read.tab, "w6:t5");
        assert_eq!(read.name, "claude");
        assert_eq!(read.status, "working");
        assert_eq!(read.seq, 5418);
        assert_eq!(read.title, "me/slingr");
        assert!(parse_agent(r#"{"error":{"code":"agent_not_found"}}"#).is_none());
    }

    #[test]
    fn only_a_stop_and_an_unseen_finish_are_worth_saying() {
        assert_eq!(Kind::of("blocked"), Some(Kind::NeedsInput));
        assert_eq!(Kind::of("done"), Some(Kind::Finished));
        for quiet in ["working", "idle", "unknown", ""] {
            assert_eq!(Kind::of(quiet), None, "{quiet}");
        }
    }

    #[test]
    fn a_stop_is_said_once() {
        let now = agent("blocked", 40);
        assert_eq!(decide(Some(&now), None, false), Act::Post(Kind::NeedsInput));
        let said = Said { kind: Kind::NeedsInput, seq: 40 };
        assert_eq!(decide(Some(&now), Some(&said), false), Act::Leave("already said"));
    }

    #[test]
    fn a_new_stop_is_said_again() {
        // The same pane stopping again later is a new state change.
        let said = Said { kind: Kind::NeedsInput, seq: 40 };
        assert_eq!(
            decide(Some(&agent("blocked", 44)), Some(&said), false),
            Act::Post(Kind::NeedsInput)
        );
        assert_eq!(
            decide(Some(&agent("done", 45)), Some(&said), false),
            Act::Post(Kind::Finished)
        );
    }

    #[test]
    fn nothing_is_said_about_what_is_in_front_of_you() {
        assert_eq!(decide(Some(&agent("blocked", 40)), None, true), Act::Leave("in front of you"));
        // And what was said about an earlier stop no longer holds.
        let older = Said { kind: Kind::Finished, seq: 31 };
        assert_eq!(decide(Some(&agent("blocked", 40)), Some(&older), true), Act::Withdraw);
    }

    #[test]
    fn an_agent_that_has_moved_on_is_taken_down() {
        let said = Said { kind: Kind::NeedsInput, seq: 40 };
        assert_eq!(decide(Some(&agent("working", 41)), Some(&said), false), Act::Withdraw);
        assert_eq!(decide(None, Some(&said), false), Act::Withdraw);
        assert_eq!(decide(Some(&agent("working", 41)), None, false), Act::Leave("moved on"));
        assert_eq!(decide(None, None, false), Act::Leave("no agent"));
    }

    #[test]
    fn a_notification_is_titled_with_the_task() {
        let note = note_for(Kind::NeedsInput, &agent("blocked", 40), Some("t/cors"));
        assert_eq!(note, Note { title: "t/cors".into(), message: "claude needs input".into() });
        let note = note_for(Kind::Finished, &agent("done", 41), Some("t/cors"));
        assert_eq!(note.message, "claude has finished");
    }

    #[test]
    fn a_tab_with_only_a_number_lends_no_title() {
        // herdr numbers a tab nobody has named, which says nothing.
        let note = note_for(Kind::NeedsInput, &agent("blocked", 40), Some("3"));
        assert_eq!(note.title, "CORS headers");
        let untitled = Agent { title: String::new(), ..agent("blocked", 40) };
        assert_eq!(note_for(Kind::NeedsInput, &untitled, None).title, "w7:p2J");
    }

    #[test]
    fn a_click_runs_answer_for_its_pane() {
        let command = click_command(Path::new("/x/slingr"), &agent("blocked", 40), None);
        assert_eq!(command, "'/x/slingr' answer 'w7:p2J' --tab 'w7:t2J'");
    }

    #[test]
    fn a_click_carries_herdrs_socket_when_herdr_gave_one() {
        let command =
            click_command(Path::new("/x/slingr"), &agent("blocked", 40), Some("/s/herdr.sock"));
        assert_eq!(
            command,
            "HERDR_SOCKET_PATH='/s/herdr.sock' '/x/slingr' answer 'w7:p2J' --tab 'w7:t2J'"
        );
    }

    #[test]
    fn quoting_survives_the_shell() {
        let nasty = "it's $HOME, `date` and \"quotes\"";
        let out = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("printf %s {}", sh_quote(nasty)))
            .output()
            .expect("sh runs");
        assert_eq!(String::from_utf8_lossy(&out.stdout), nasty);
    }

    #[test]
    fn a_notification_goes_to_its_click_and_brings_the_terminal_forward() {
        let note = Note { title: "t/cors".into(), message: "claude needs input".into() };
        let args = post_args(&note, "slingr.w7:p2J", "click", Some("com.mitchellh.ghostty"));
        assert_eq!(
            args,
            [
                "-title", "t/cors", "-message", "claude needs input", "-group", "slingr.w7:p2J",
                "-execute", "click", "-activate", "com.mitchellh.ghostty",
            ]
        );
        let args = post_args(&note, "slingr.w7:p2J", "click", None);
        assert!(!args.contains(&"-activate".to_string()));
    }

    #[test]
    fn finds_the_terminal_as_herdr_does() {
        assert_eq!(terminal_bundle(Some("ghostty"), None).as_deref(), Some("com.mitchellh.ghostty"));
        assert_eq!(
            terminal_bundle(Some("iTerm.app"), Some("com.mitchellh.ghostty")).as_deref(),
            Some("com.googlecode.iterm2")
        );
        // A terminal herdr does not know still started the shell herdr runs in.
        assert_eq!(
            terminal_bundle(Some("rio"), Some("com.raphaelamorim.rio")).as_deref(),
            Some("com.raphaelamorim.rio")
        );
        assert_eq!(terminal_bundle(None, None), None);
    }

    #[test]
    fn reads_the_bundle_id_lsappinfo_prints() {
        assert_eq!(
            parse_bundle_id("\"CFBundleIdentifier\"=\"com.mitchellh.ghostty\"\n").as_deref(),
            Some("com.mitchellh.ghostty")
        );
        assert_eq!(parse_bundle_id(""), None);
        assert_eq!(parse_bundle_id("\"CFBundleIdentifier\"=[ NULL ]"), None);
    }
}
