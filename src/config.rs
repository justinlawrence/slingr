//! The task taxonomy, read from `~/.config/sling/workspaces.toml`.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;

/// Shipped defaults, written out on first run so there is always a file to edit.
const DEFAULT: &str = include_str!("../workspaces.toml");

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub known: Vec<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub order: Order,
}

#[derive(Debug, Default, Deserialize)]
pub struct Order {
    #[serde(default)]
    pub prefixes: Vec<String>,
}

pub fn config_dir() -> PathBuf {
    home().join(".config/slingr")
}

pub fn config_path() -> PathBuf {
    config_dir().join("workspaces.toml")
}

pub fn home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/"))
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = config_path();
        if !path.exists() {
            fs::create_dir_all(config_dir())?;
            fs::write(&path, DEFAULT)
                .with_context(|| format!("seeding {}", path.display()))?;
        }
        let text = fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }
}
