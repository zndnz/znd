//! 利用者が「自己責任で許可」したプラグインの記録。
//!
//! 危険度が高いものは既定では読み込まず、利用者が明示的に許可したものだけを読み込む。
//! 許可は中身の SHA-256 に結び付けるので、ファイルが差し替えられると許可は無効になる。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// 許可の記録の保存先。環境変数 `ZND_PLUGIN_TRUST` で変えられる（テスト用）。
pub fn trust_file() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ZND_PLUGIN_TRUST") {
        return Some(PathBuf::from(p));
    }
    if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("znd").join("plugin-trust.json"))
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/znd/plugin-trust.json"))
    } else {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/znd/plugin-trust.json"))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrustEntry {
    pub path: String,
    pub level: String,
}

fn store() -> &'static Mutex<BTreeMap<String, TrustEntry>> {
    static S: OnceLock<Mutex<BTreeMap<String, TrustEntry>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(read()))
}

fn read() -> BTreeMap<String, TrustEntry> {
    let Some(file) = trust_file() else { return BTreeMap::new() };
    let Ok(text) = std::fs::read_to_string(file) else { return BTreeMap::new() };
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(&text) else { return BTreeMap::new() };
    map.into_iter()
        .map(|(hash, v)| {
            let get = |k: &str| v.get(k).and_then(|s| s.as_str()).unwrap_or("").to_string();
            (hash, TrustEntry { path: get("path"), level: get("level") })
        })
        .collect()
}

fn write(map: &BTreeMap<String, TrustEntry>) -> Result<(), String> {
    let Some(file) = trust_file() else { return Err("許可の記録を保存する場所がありません".into()) };
    let json: serde_json::Map<String, serde_json::Value> = map
        .iter()
        .map(|(h, e)| (h.clone(), serde_json::json!({ "path": e.path, "level": e.level })))
        .collect();
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&file, serde_json::to_string_pretty(&json).unwrap_or_default()).map_err(|e| format!("{}: {e}", file.display()))
}

/// この中身（ハッシュ）が許可されているか。
pub fn is_trusted(hash: &str) -> bool {
    store().lock().unwrap().contains_key(hash)
}

/// 自己責任での許可を記録する。
pub fn allow(hash: &str, path: &Path, level: &str) -> Result<(), String> {
    let mut map = store().lock().unwrap();
    map.insert(hash.to_string(), TrustEntry { path: path.display().to_string(), level: level.to_string() });
    write(&map)
}

/// 許可を取り消す。
pub fn revoke(hash: &str) -> Result<(), String> {
    let mut map = store().lock().unwrap();
    map.remove(hash);
    write(&map)
}

pub fn entries() -> Vec<(String, TrustEntry)> {
    store().lock().unwrap().iter().map(|(h, e)| (h.clone(), e.clone())).collect()
}
