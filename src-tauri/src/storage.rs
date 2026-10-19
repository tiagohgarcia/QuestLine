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
