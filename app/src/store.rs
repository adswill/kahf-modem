//! What the window keeps between runs besides the settings: conversations, mail, heard stations.

use std::fs;
use std::path::Path;

use crate::model::Saved;

const FILE: &str = "ui-state.json";

pub fn load(dir: &Path) -> Saved {
    fs::read_to_string(dir.join(FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Writes through a temporary file so a crash never leaves half a file.
pub fn save(dir: &Path, saved: &Saved) {
    if fs::create_dir_all(dir).is_err() {
        return;
    }
    let Ok(text) = serde_json::to_string(saved) else { return };
    let tmp = dir.join(format!("{FILE}.tmp"));
    if fs::write(&tmp, text).is_ok() {
        let _ = fs::rename(&tmp, dir.join(FILE));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_saved_loads_empty_and_a_save_comes_back() {
        let dir = std::env::temp_dir().join(format!("kahf-store-{}", std::process::id()));
        assert!(load(&dir).messages.is_empty());
        let mut s = Saved::default();
        s.heard.push(crate::types::Heard { call: "K1AB".into(), ..Default::default() });
        save(&dir, &s);
        assert_eq!(load(&dir).heard[0].call, "K1AB");
        let _ = fs::remove_dir_all(&dir);
    }
}
