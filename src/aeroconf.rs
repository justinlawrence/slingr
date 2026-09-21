//! Writing `persistent-workspaces` into AeroSpace's own config.
//!
//! AeroSpace forgets a workspace once it is empty and off screen. `known` and
//! `seen.json` existed to work around that: lists sling kept so a task could be
//! offered before anything was in it. `persistent-workspaces` does the job
//! properly — the workspace genuinely exists, so it appears in AeroSpace's menu
//! bar, can be switched to, and needs no remembering on our side.
//!
//! It is static config, so sling maintains it. Only the block between the
//! markers is touched; the rest of the file is left exactly as it was.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::config::home;

pub const BEGIN: &str = "# slingr:begin — managed by `slingr sync`, edits here are overwritten";
pub const END: &str = "# slingr:end";

pub fn path() -> PathBuf {
    home().join(".aerospace.toml")
}

/// Remove a `persistent-workspaces` array that is not inside the managed
/// block. TOML rejects the whole file for a duplicate key, so a hand-written
/// one has to be absorbed rather than added to.
fn without_unmanaged(text: &str) -> String {
    let managed_from = text.find(BEGIN);
    let managed_to = text.find(END).map(|at| at + END.len());

    let mut out = String::with_capacity(text.len());
    let mut offset = 0usize;
    let mut skipping = false;
    for line in text.lines() {
        let start = offset;
        offset += line.len() + 1;

        let inside_managed = matches!((managed_from, managed_to), (Some(f), Some(t)) if start >= f && start < t);
        if skipping {
            // The array ends at the first line that closes it.
            if line.trim_start().starts_with(']') {
                skipping = false;
            }
            continue;
        }
        if !inside_managed && line.trim_start().starts_with("persistent-workspaces") {
            // Single line form closes on the same line.
            skipping = !line.contains(']');
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Rewrite the managed block, or append one if it is not there yet.
/// Returns the new file contents, so the caller can decide whether to write.
pub fn with_workspaces(existing: &str, names: &[String]) -> String {
    let existing = &without_unmanaged(existing);
    let mut block = String::from(BEGIN);
    block.push_str("\npersistent-workspaces = [\n");
    for name in names {
        block.push_str(&format!("  \"{name}\",\n"));
    }
    block.push_str("]\n");
    block.push_str(END);

    match (existing.find(BEGIN), existing.find(END)) {
        (Some(from), Some(to)) if to > from => {
            let mut out = String::with_capacity(existing.len() + block.len());
            out.push_str(&existing[..from]);
            out.push_str(&block);
            out.push_str(&existing[to + END.len()..]);
            out
        }
        _ => {
            // No block yet. It has to go above the first table header:
            // appended to the end it lands inside whatever section is last,
            // and `persistent-workspaces` inside `[mode.service.binding]`
            // parses as a keybinding whose modifiers make no sense.
            let at = existing
                .lines()
                .scan(0usize, |offset, line| {
                    let start = *offset;
                    *offset += line.len() + 1;
                    Some((start, line))
                })
                .find(|(_, line)| line.trim_start().starts_with('['))
                .map(|(start, _)| start);

            match at {
                Some(start) => format!("{}{block}\n\n{}", &existing[..start], &existing[start..]),
                None => format!("{}\n\n{block}\n", existing.trim_end()),
            }
        }
    }
}

/// True if anything changed.
pub fn sync(names: &[String]) -> Result<bool> {
    let file = path();
    let existing = fs::read_to_string(&file)
        .with_context(|| format!("reading {}", file.display()))?;
    let updated = with_workspaces(&existing, names);
    if updated == existing {
        return Ok(false);
    }
    fs::write(&file, updated).with_context(|| format!("writing {}", file.display()))?;
    Ok(true)
}

/// Does the config say version 2? `persistent-workspaces` is ignored without
/// it, silently, which is a bad way to find out.
pub fn declares_version_2(text: &str) -> bool {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .any(|l| l.starts_with("config-version") && l.contains('2'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn adds_a_block_when_there_is_none() {
        let out = with_workspaces("start-at-login = false\n", &names(&["t-mail"]));
        assert!(out.starts_with("start-at-login = false"));
        assert!(out.contains("persistent-workspaces = [\n  \"t-mail\",\n]"));
        assert!(out.contains(BEGIN) && out.contains(END));
    }

    #[test]
    fn replaces_only_its_own_block() {
        let before = format!(
            "before = 1\n\n{BEGIN}\npersistent-workspaces = [\n  \"old\",\n]\n{END}\n\nafter = 2\n"
        );
        let out = with_workspaces(&before, &names(&["new-one", "new-two"]));
        assert!(out.contains("before = 1"), "{out}");
        assert!(out.contains("after = 2"), "{out}");
        assert!(out.contains("\"new-one\""), "{out}");
        assert!(!out.contains("\"old\""), "{out}");
        assert_eq!(out.matches(BEGIN).count(), 1);
    }

    #[test]
    fn absorbs_a_hand_written_list_rather_than_duplicating_it() {
        // TOML rejects the whole file for a duplicate key, and AeroSpace then
        // silently keeps its previous config.
        let before = "x = 1\npersistent-workspaces = [\n  \"old\",\n]\n\n[mode.main.binding]\n";
        let out = with_workspaces(before, &names(&["t-mail"]));
        assert_eq!(out.matches("persistent-workspaces").count(), 1, "{out}");
        assert!(!out.contains("\"old\""), "{out}");
        assert!(out.contains("x = 1") && out.contains("[mode.main.binding]"), "{out}");
    }

    #[test]
    fn absorbs_a_single_line_list_too() {
        let out = with_workspaces("persistent-workspaces = [\"old\"]\nx = 1\n", &names(&["a"]));
        assert_eq!(out.matches("persistent-workspaces").count(), 1, "{out}");
        assert!(out.contains("x = 1"), "{out}");
    }

    #[test]
    fn goes_above_the_first_table_header() {
        // Below one it would be read as belonging to that section — a
        // persistent-workspaces key inside [mode.service.binding] is a
        // keybinding with unparseable modifiers.
        let before = "start-at-login = false\n\n[mode.main.binding]\nalt-1 = 'workspace 1'\n";
        let out = with_workspaces(before, &names(&["t-mail"]));

        let block_at = out.find(BEGIN).expect("block written");
        let section_at = out.find("[mode.main.binding]").expect("section kept");
        assert!(block_at < section_at, "{out}");
        assert!(out.contains("start-at-login = false"), "{out}");
        assert!(out.contains("alt-1 = 'workspace 1'"), "{out}");
    }

    #[test]
    fn rewriting_twice_changes_nothing_the_second_time() {
        let once = with_workspaces("x = 1\n", &names(&["a", "b"]));
        let twice = with_workspaces(&once, &names(&["a", "b"]));
        assert_eq!(once, twice);
    }

    #[test]
    fn notices_whether_the_option_will_be_honoured() {
        assert!(declares_version_2("config-version = 2\n"));
        assert!(!declares_version_2("start-at-login = false\n"));
        // A comment about it is not a declaration of it.
        assert!(!declares_version_2("# config-version = 2\n"));
    }
}
