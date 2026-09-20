//! Colours, inherited rather than invented.
//!
//! Omarchy plugins are theme-aware by convention: they follow the system
//! theme instead of shipping a palette. There is no system theme to read on
//! macOS, but there is something better — the terminal this all revolves
//! around. Taking Ghostty's colours means the panel matches the windows it
//! appears over, and changes when they do.

use std::path::PathBuf;

use serde::Serialize;

/// Ghostty's own defaults, used when there is nothing to read.
const FALLBACK: Theme = Theme {
    background: Cow::Borrowed("#11111b"),
    foreground: Cow::Borrowed("#cdd6f4"),
    accent: Cow::Borrowed("#89b4fa"),
    ok: Cow::Borrowed("#a6e3a1"),
    warn: Cow::Borrowed("#fab387"),
    dim: Cow::Borrowed("#6c7086"),
};

use std::borrow::Cow;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Theme {
    pub background: Cow<'static, str>,
    pub foreground: Cow<'static, str>,
    pub accent: Cow<'static, str>,
    pub ok: Cow<'static, str>,
    pub warn: Cow<'static, str>,
    pub dim: Cow<'static, str>,
}

impl Default for Theme {
    fn default() -> Self {
        FALLBACK.clone()
    }
}

fn config_path() -> PathBuf {
    crate::config::home().join(".config/ghostty/config")
}

/// Read what we can; anything missing keeps its fallback.
pub fn current() -> Theme {
    std::fs::read_to_string(config_path())
        .map(|text| parse(&text))
        .unwrap_or_default()
}

/// Ghostty's config is `key = value`, with the 16 ANSI colours given as
/// `palette = N=#rrggbb`. Only the few entries that matter are read; anything
/// unparseable simply keeps its fallback rather than failing.
pub fn parse(text: &str) -> Theme {
    let mut theme = Theme::default();
    let mut palette: [Option<String>; 16] = Default::default();

    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim(), value.trim());
        match key {
            "background" => theme.background = Cow::Owned(normalise(value)),
            "foreground" => theme.foreground = Cow::Owned(normalise(value)),
            "palette" => {
                if let Some((index, colour)) = value.split_once('=') {
                    if let Ok(i) = index.trim().parse::<usize>() {
                        if i < 16 {
                            palette[i] = Some(normalise(colour.trim()));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Bright variants where there is one, since the panel sits on a dark
    // ground and the normal shades go muddy.
    if let Some(blue) = palette[12].clone().or_else(|| palette[4].clone()) {
        theme.accent = Cow::Owned(blue);
    }
    if let Some(green) = palette[10].clone().or_else(|| palette[2].clone()) {
        theme.ok = Cow::Owned(green);
    }
    if let Some(yellow) = palette[11].clone().or_else(|| palette[3].clone()) {
        theme.warn = Cow::Owned(yellow);
    }
    // Bright black is the conventional "dim but legible" slot.
    if let Some(grey) = palette[8].clone().or_else(|| palette[0].clone()) {
        theme.dim = Cow::Owned(grey);
    }
    theme
}

fn normalise(value: &str) -> String {
    let v = value.trim().trim_matches('"');
    if v.starts_with('#') { v.to_string() } else { format!("#{v}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GHOSTTY: &str = r#"
# a comment
font-size 		= 17
background = #010409
foreground = #e6edf3
palette = 0=#484f58
palette = 2=#3fb950
palette = 4=#58a6ff
palette = 8=#6e7681
palette = 10=#56d364
palette = 11=#e3b341
palette = 12=#79c0ff
"#;

    #[test]
    fn takes_the_terminal_colours() {
        let t = parse(GHOSTTY);
        assert_eq!(t.background, "#010409");
        assert_eq!(t.foreground, "#e6edf3");
    }

    #[test]
    fn prefers_the_bright_variant_of_each_colour() {
        let t = parse(GHOSTTY);
        assert_eq!(t.accent, "#79c0ff", "bright blue over normal blue");
        assert_eq!(t.ok, "#56d364", "bright green over normal green");
        assert_eq!(t.dim, "#6e7681", "bright black is the legible grey");
    }

    #[test]
    fn falls_back_to_the_normal_shade_when_there_is_no_bright_one() {
        let t = parse("palette = 4=#58a6ff\npalette = 2=#3fb950\n");
        assert_eq!(t.accent, "#58a6ff");
        assert_eq!(t.ok, "#3fb950");
    }

    #[test]
    fn keeps_its_defaults_when_there_is_nothing_to_read() {
        assert_eq!(parse(""), Theme::default());
        assert_eq!(parse("nonsense without an equals sign"), Theme::default());
    }

    #[test]
    fn tolerates_a_colour_written_without_its_hash() {
        assert_eq!(parse("background = 010409").background, "#010409");
    }
}
