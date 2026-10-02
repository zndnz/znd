//! 制作ログ: 作業時間の記録と、コンポ別の統計。上映後に制作過程を振り返るための材料。

use crate::effect::effect_def;
use crate::model::{LayerKind, Project};
use crate::visit::visit_layer;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const LOG_FILE: &str = ".znd-log.jsonl";

/// 作業セッション 1 回ぶんの記録。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionEntry {
    /// 開始時刻（UNIX 秒）。
    pub started: u64,
    /// 作業していた秒数（アプリを開いていて、操作が続いていた時間）。
    pub seconds: u64,
    /// 編集操作の回数。
    pub edits: u32,
    #[serde(default)]
    pub user: String,
}

fn log_path(project_path: &Path) -> PathBuf {
    let stem = project_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "untitled".into());
    project_path.parent().unwrap_or(Path::new(".")).join(format!("{stem}{LOG_FILE}"))
}

/// セッションを追記する（1 行 1 件の JSON。追記のみなので、複数人が同時に書いても壊れにくい）。
pub fn append_session(project_path: &Path, entry: &SessionEntry) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(log_path(project_path))?;
    writeln!(f, "{}", serde_json::to_string(entry).unwrap_or_default())
}

pub fn read_sessions(project_path: &Path) -> Vec<SessionEntry> {
    let Ok(text) = std::fs::read_to_string(log_path(project_path)) else { return Vec::new() };
    text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CompStats {
    pub name: String,
    pub layers: usize,
    /// レイヤー種別（日本語名）ごとの枚数。
    pub by_kind: BTreeMap<&'static str, usize>,
    /// エフェクト（日本語名）ごとの使用回数。
    pub effects: BTreeMap<String, usize>,
    pub keyframes: usize,
    pub expressions: usize,
    pub masks: usize,
}

pub fn kind_label(kind: &LayerKind) -> &'static str {
    match kind {
        LayerKind::Solid { .. } => "平面",
        LayerKind::Footage { .. } => "フッテージ",
        LayerKind::Precomp { .. } => "プリコンポ",
        LayerKind::Null => "ヌル",
        LayerKind::Adjustment => "調整レイヤー",
        LayerKind::Shape { .. } => "シェイプ",
        LayerKind::Text { .. } => "テキスト",
        LayerKind::Audio { .. } => "オーディオ",
        LayerKind::Camera { .. } => "カメラ",
        LayerKind::Light { .. } => "ライト",
        LayerKind::Model { .. } => "3D モデル",
    }
}

pub fn comp_stats(project: &Project) -> Vec<CompStats> {
    project
        .compositions
        .iter()
        .map(|c| {
            let mut s = CompStats { name: c.name.clone(), layers: c.layers.len(), ..Default::default() };
            for l in &c.layers {
                *s.by_kind.entry(kind_label(&l.kind)).or_default() += 1;
                s.masks += l.masks.len();
                for fx in &l.effects {
                    let label = effect_def(&fx.kind).map(|d| d.label.to_string()).unwrap_or_else(|| fx.kind.clone());
                    *s.effects.entry(label).or_default() += 1;
                }
                visit_layer(l, &mut |_, p| {
                    s.keyframes += p.key_times().len();
                    s.expressions += p.has_expression() as usize;
                });
            }
            s
        })
        .collect()
}

/// 振り返り用のまとめ（プレーンテキスト）。
pub fn summary_text(project: &Project, sessions: &[SessionEntry]) -> String {
    let total: u64 = sessions.iter().map(|s| s.seconds).sum();
    let edits: u64 = sessions.iter().map(|s| s.edits as u64).sum();
    let mut out = format!(
        "■ 制作ログ「{}」\n作業時間: {} 時間 {} 分（{} セッション、編集 {} 回）\n",
        project.name,
        total / 3600,
        total % 3600 / 60,
        sessions.len(),
        edits
    );
    let mut users: BTreeMap<&str, u64> = BTreeMap::new();
    for s in sessions.iter().filter(|s| !s.user.is_empty()) {
        *users.entry(&s.user).or_default() += s.seconds;
    }
    for (user, secs) in users {
        out += &format!("  {user}: {} 時間 {} 分\n", secs / 3600, secs % 3600 / 60);
    }
    for s in comp_stats(project) {
        out += &format!(
            "\n● コンポ「{}」 レイヤー {} 枚 / キーフレーム {} 個 / エクスプレッション {} 個 / マスク {} 個\n",
            s.name, s.layers, s.keyframes, s.expressions, s.masks
        );
        let kinds: Vec<String> = s.by_kind.iter().map(|(k, n)| format!("{k} {n}")).collect();
        if !kinds.is_empty() {
            out += &format!("  内訳: {}\n", kinds.join("、"));
        }
        let mut fx: Vec<(&String, &usize)> = s.effects.iter().collect();
        fx.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        if !fx.is_empty() {
            out += &format!("  エフェクト: {}\n", fx.iter().map(|(k, n)| format!("{k} ×{n}")).collect::<Vec<_>>().join("、"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::demo_project;
    use crate::effect::EffectInstance;
    use crate::property::Expression;

    #[test]
    fn sessions_append_and_read_back() {
        let dir = std::env::temp_dir().join(format!("znd-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("作品.znd.json");
        assert!(read_sessions(&path).is_empty());
        let a = SessionEntry { started: 1_790_000_000, seconds: 5400, edits: 120, user: "あい".into() };
        let b = SessionEntry { started: 1_790_010_000, seconds: 1900, edits: 30, user: "うえ".into() };
        append_session(&path, &a).unwrap();
        append_session(&path, &b).unwrap();
        assert_eq!(read_sessions(&path), vec![a.clone(), b.clone()]);
        assert!(dir.join("作品.znd.znd-log.jsonl").exists());
        let text = summary_text(&demo_project(), &[a, b]);
        assert!(text.contains("作業時間: 2 時間 1 分（2 セッション、編集 150 回）"), "{text}");
        assert!(text.contains("あい: 1 時間 30 分"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stats_count_kinds_effects_keys_and_expressions() {
        let mut p = demo_project();
        let l = &mut p.compositions[0].layers[0];
        l.effects.push(EffectInstance::new("glow", 100.0, 100.0).unwrap());
        l.effects.push(EffectInstance::new("glow", 100.0, 100.0).unwrap());
        l.effects.push(EffectInstance::new("gaussian_blur", 100.0, 100.0).unwrap());
        l.transform.rotation.expression = Some(Expression { source: "time".into(), enabled: true });
        let stats = comp_stats(&p);
        let main = &stats[0];
        assert_eq!(main.layers, 9);
        assert_eq!(main.by_kind["平面"], 7);
        assert_eq!(main.by_kind["ヌル"], 1);
        assert_eq!(main.by_kind["プリコンポ"], 1);
        assert_eq!(main.effects["グロー"], 2);
        assert_eq!(main.expressions, 1);
        assert!(main.keyframes > 20);
        let text = summary_text(&p, &[]);
        assert!(text.contains("エフェクト: グロー ×2、ブラー（ガウス） ×1"), "{text}");
    }
}
