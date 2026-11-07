//! 利用者ごとの設定（プロジェクトには保存しないもの）。znd-app と同じ設定フォルダの `prefs.json` を使う。
//!
//! znd-app だけが使う項目（イージングのお気に入りなど）も消さないよう、読んだ JSON をそのまま持ち回し、
//! ここで扱う項目だけを書き換えて保存する。

use serde_json::{Map, Value};
use std::path::PathBuf;

/// 設定フォルダ（znd-app と同じ場所）。`ZND_CONFIG_DIR` で変えられる。
pub fn config_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("ZND_CONFIG_DIR") {
        return Some(PathBuf::from(d));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        home.map(|h| h.join("Library/Application Support/znd"))
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("znd"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| home.map(|h| h.join(".config"))).map(|d| d.join("znd"))
    }
}

#[derive(Default, Clone)]
pub struct Prefs {
    raw: Map<String, Value>,
}

impl Prefs {
    fn path() -> Option<PathBuf> {
        config_dir().map(|d| d.join("prefs.json"))
    }

    pub fn load() -> Prefs {
        let raw = Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| match v {
                Value::Object(m) => Some(m),
                _ => None,
            })
            .unwrap_or_default();
        Prefs { raw }
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path().ok_or("設定フォルダが分かりません")?;
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(&Value::Object(self.raw.clone())).unwrap_or_default();
        std::fs::write(&path, text).map_err(|e| e.to_string())
    }

    /// サンドボックスの外で動くネイティブのプラグイン（znd ABI・OpenFX）も読み込む。既定は false。
    pub fn allow_native_plugins(&self) -> bool {
        self.raw.get("allow_native_plugins").and_then(Value::as_bool).unwrap_or(false)
    }

    pub fn set_allow_native_plugins(&mut self, on: bool) {
        self.raw.insert("allow_native_plugins".into(), Value::Bool(on));
    }

    /// 映像と音のずれの補正（ミリ秒）。正の値で映像を遅らせる。
    pub fn av_offset_ms(&self) -> f64 {
        self.raw.get("av_offset_ms").and_then(Value::as_f64).unwrap_or(0.0)
    }

    pub fn set_av_offset_ms(&mut self, ms: f64) {
        self.raw.insert("av_offset_ms".into(), serde_json::json!(ms));
    }

    /// 画面（TypeScript）が使う項目。
    pub fn get(&self, key: &str) -> Value {
        self.raw.get(key).cloned().unwrap_or(Value::Null)
    }

    pub fn set(&mut self, key: &str, value: Value) {
        if value.is_null() {
            self.raw.remove(key);
        } else {
            self.raw.insert(key.to_string(), value);
        }
    }
}
