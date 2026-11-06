//! 利用者ごとの設定（プロジェクトには保存しないもの）。設定フォルダの `prefs.json`。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub(crate) struct Prefs {
    /// 映像と音のずれの補正（ミリ秒）。正の値で映像を遅らせる（Bluetooth のイヤホンなど、音が遅れて聞こえる環境向け）。
    #[serde(default)]
    pub av_offset_ms: f64,
    /// イージングのお気に入り。
    #[serde(default)]
    pub ease_favorites: Vec<crate::easing::FavoriteEase>,
}

impl Prefs {
    fn path() -> Option<std::path::PathBuf> {
        crate::keymap::config_dir().map(|d| d.join("prefs.json"))
    }

    pub fn load() -> Prefs {
        Self::path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path().ok_or("設定フォルダが分かりません")?;
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, serde_json::to_string_pretty(self).unwrap_or_default()).map_err(|e| e.to_string())
    }
}
