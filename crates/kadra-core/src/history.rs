//! Recent captures, persisted between sessions.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Number of captures remembered.
pub const CAPACITY: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub path: PathBuf,
    /// Seconds since the Unix epoch.
    pub taken_at: u64,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    #[serde(default)]
    entries: VecDeque<Entry>,
}

impl History {
    /// Loads the history; a missing or unreadable file gives an empty one.
    pub fn load(path: &Path) -> Self {
        let mut history: Self = fs::read_to_string(path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default();
        history.entries.truncate(CAPACITY);
        history
    }

    /// Writes the history atomically.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let text = toml::to_string(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, text)?;
        fs::rename(&tmp, path)
    }

    /// Records a capture as the most recent one.
    pub fn push(&mut self, entry: Entry) {
        self.entries.retain(|e| e.path != entry.path);
        self.entries.push_front(entry);
        self.entries.truncate(CAPACITY);
    }

    /// Forgets files that no longer exist (deleted or moved by the user).
    pub fn prune_missing(&mut self) {
        self.entries.retain(|e| e.path.exists());
    }

    /// Captures from newest to oldest.
    pub fn recent(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, taken_at: u64) -> Entry {
        Entry {
            path: PathBuf::from(name),
            taken_at,
            width: 10,
            height: 20,
        }
    }

    #[test]
    fn newest_first_without_duplicates() {
        let mut h = History::default();
        h.push(entry("a.png", 1));
        h.push(entry("b.png", 2));
        h.push(entry("a.png", 3));
        let names: Vec<_> = h.recent().map(|e| e.path.to_str().unwrap()).collect();
        assert_eq!(names, ["a.png", "b.png"]);
        assert_eq!(h.recent().next().unwrap().taken_at, 3);
    }

    #[test]
    fn capacity_is_bounded() {
        let mut h = History::default();
        for i in 0..(CAPACITY as u64 + 10) {
            h.push(entry(&format!("{i}.png"), i));
        }
        assert_eq!(h.len(), CAPACITY);
        assert_eq!(h.recent().next().unwrap().taken_at, CAPACITY as u64 + 9);
    }

    #[test]
    fn round_trip_and_prune() {
        let dir = tempfile::tempdir().unwrap();
        let kept = dir.path().join("kept.png");
        fs::write(&kept, b"x").unwrap();

        let mut h = History::default();
        h.push(entry(dir.path().join("gone.png").to_str().unwrap(), 1));
        h.push(Entry {
            path: kept.clone(),
            ..entry("", 2)
        });
        let file = dir.path().join("history.toml");
        h.save(&file).unwrap();

        let mut loaded = History::load(&file);
        assert_eq!(loaded, h);
        loaded.prune_missing();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.recent().next().unwrap().path, kept);
    }

    #[test]
    fn unreadable_file_gives_empty_history() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("history.toml");
        fs::write(&file, "not = [valid").unwrap();
        assert!(History::load(&file).is_empty());
        assert!(History::load(&dir.path().join("missing.toml")).is_empty());
    }
}
