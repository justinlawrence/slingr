//! Dialogs, drawn through System Events.
//!
//! sling is launched by AeroSpace's `exec-and-forget`, which has no GUI
//! context, so a bare `osascript` dialog never appears. Handing the dialog to
//! System Events — an application already running that can come to the front —
//! is what puts it on screen.

use std::process::Command;

/// Joins a multi-selection on the way back. A plain list comes back
/// comma-separated, which is ambiguous the moment a window title contains a
/// comma — and plenty do.
const UNIT_SEPARATOR: char = '\u{1f}';

pub trait Prompt {
    fn choose(&self, items: &[String], title: &str, prompt: &str) -> Option<String>;

    /// The same question, with the rows still in pieces.
    ///
    /// A front end that can lay out its own columns wants the parts; the
    /// AppleScript dialog can only take flat strings, so the default renders
    /// them and falls back. Callers should prefer this.
    fn choose_rows(&self, rows: &[crate::picker::Row], title: &str, subtitle: &str) -> Option<String> {
        let items: Vec<String> = rows.iter().map(crate::picker::render_row).collect();
        let chosen = self.choose(&items, title, subtitle)?;
        crate::picker::row_id_for(rows, &chosen)
    }

    fn choose_many_rows(
        &self,
        rows: &[crate::picker::Row],
        title: &str,
        subtitle: &str,
    ) -> Option<Vec<String>> {
        let items: Vec<String> = rows.iter().map(crate::picker::render_row).collect();
        let chosen = self.choose_many(&items, title, subtitle)?;
        Some(chosen.iter().filter_map(|c| crate::picker::row_id_for(rows, c)).collect())
    }

    /// Several lines at once. `None` when cancelled or nothing was selected.
    fn choose_many(&self, items: &[String], title: &str, prompt: &str) -> Option<Vec<String>>;
    fn ask_text(&self, title: &str, prompt: &str) -> Option<String>;
}

/// Escape a string for interpolation into an AppleScript literal.
///
/// Window titles carry quotes and the occasional backslash. Unescaped, they end
/// the literal early, the script fails to compile, and the dialog silently
/// never appears.
pub fn applescript_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[derive(Default)]
pub struct SystemEvents;

impl SystemEvents {
    fn osascript(&self, script: &str) -> Option<String> {
        let out = Command::new("osascript").arg("-e").arg(script).output().ok()?;
        if !out.status.success() {
            return None;
        }
        // Only the trailing newline: leading spaces are meaningful, they are
        // how an unoccupied workspace is indented in the menu.
        let text = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }
}

impl Prompt for SystemEvents {
    fn choose(&self, items: &[String], title: &str, prompt: &str) -> Option<String> {
        let listing = items
            .iter()
            .map(|i| format!("\"{}\"", applescript_string(i)))
            .collect::<Vec<_>>()
            .join(", ");
        self.osascript(&format!(
            "tell application \"System Events\"\n\
               activate\n\
               set r to choose from list {{{listing}}} with title \"{}\" with prompt \"{}\" \
               OK button name \"Sling\" cancel button name \"Cancel\"\n\
             end tell\n\
             if r is false then return \"\"\n\
             return item 1 of r",
            applescript_string(title),
            applescript_string(prompt),
        ))
    }

    fn choose_many(&self, items: &[String], title: &str, prompt: &str) -> Option<Vec<String>> {
        let listing = items
            .iter()
            .map(|i| format!("\"{}\"", applescript_string(i)))
            .collect::<Vec<_>>()
            .join(", ");
        let out = self.osascript(&format!(
            "tell application \"System Events\"\n\
               activate\n\
               set chosen to choose from list {{{listing}}} with title \"{}\" with prompt \"{}\" \
               OK button name \"Next\" cancel button name \"Cancel\" with multiple selections allowed\n\
             end tell\n\
             if chosen is false then return \"\"\n\
             set AppleScript's text item delimiters to (ASCII character 31)\n\
             set out to chosen as text\n\
             set AppleScript's text item delimiters to \"\"\n\
             return out",
            applescript_string(title),
            applescript_string(prompt),
        ))?;
        let picked: Vec<String> = out
            .split(UNIT_SEPARATOR)
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();
        if picked.is_empty() {
            None
        } else {
            Some(picked)
        }
    }

    fn ask_text(&self, title: &str, prompt: &str) -> Option<String> {
        self.osascript(&format!(
            "tell application \"System Events\"\n\
               activate\n\
               set d to display dialog \"{}\" with title \"{}\" default answer \"\" \
               buttons {{\"Cancel\", \"Create\"}} default button 2\n\
             end tell\n\
             if button returned of d is \"Cancel\" then return \"\"\n\
             return text returned of d",
            applescript_string(prompt),
            applescript_string(title),
        ))
    }
}
