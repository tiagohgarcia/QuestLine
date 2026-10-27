use crate::model::Store;
use std::fs;
use std::path::{Path, PathBuf};

/// All data lives in `<project>/data` unless QUESTLINE_DATA_DIR is set.
pub fn data_file() -> PathBuf {
    let dir = std::env::var_os("QUESTLINE_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("data"));
    dir.join("questline.json")
}

pub fn load(path: &Path) -> Store {
    let Ok(text) = fs::read_to_string(path) else {
        return Store::default();
    };
    match serde_json::from_str(&text) {
        Ok(store) => store,
        Err(err) => {
            // Never silently drop data: keep the unreadable file next to the new one.
            let backup = path.with_extension(format!("corrupt-{}.json", chrono::Utc::now().timestamp()));
            eprintln!("Could not parse {}: {err}. Moved it to {}.", path.display(), backup.display());
            let _ = fs::rename(path, &backup);
            Store::default()
        }
    }
}

/// Writes to a temp file first so a crash never leaves a half-written save.
pub fn save(path: &Path, store: &Store) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
    fs::write(&tmp, json).map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Goal, GoalKind};

    /// A fresh, empty folder per test under the system temp dir.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("questline-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample() -> Store {
        let mut store = Store::default();
        store.next_id = 2;
        store.profile.xp = 42;
        store.goals.push(Goal {
            id: 1,
            name: "Read".into(),
            category: "Books".into(),
            kind: GoalKind::Counter { target: 12.0, unit: "books".into() },
            created_at: chrono::Utc::now(),
            deadline: None,
            baseline: 3.0,
            entries: Vec::new(),
            completed_at: None,
            milestones_hit: vec![25],
            next_entry_id: 0,
            archived_at: None,
        });
        store
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = scratch("round-trip");
        let path = dir.join("nested").join("questline.json");
        save(&path, &sample()).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.next_id, 2);
        assert_eq!(loaded.profile.xp, 42);
        assert_eq!(loaded.goals[0].name, "Read");
        assert_eq!(loaded.goals[0].milestones_hit, [25]);
        assert!(!path.with_extension("json.tmp").exists(), "temp file is renamed away");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_file_loads_empty_store() {
        let dir = scratch("missing");
        let store = load(&dir.join("questline.json"));
        assert_eq!(store.next_id, 1);
        assert!(store.goals.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn corrupt_file_is_kept_as_backup() {
        let dir = scratch("corrupt");
        let path = dir.join("questline.json");
        fs::write(&path, "{ not json").unwrap();

        let store = load(&path);
        assert!(store.goals.is_empty());
        assert!(!path.exists());
        let backups: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.starts_with("questline.corrupt-"))
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read_to_string(dir.join(&backups[0])).unwrap(), "{ not json");
        fs::remove_dir_all(dir).unwrap();
    }
}
