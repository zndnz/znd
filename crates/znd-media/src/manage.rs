//! 素材管理: 欠落フッテージ・欠落フォントの検出、一括再リンク、ファイル収集。

use crate::footage::resolve_path;
use crate::MediaError;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use znd_core::{rights, CompId, FootageId, Project};

/// ファイルが見つからないフッテージ。
pub fn missing_footage(project: &Project, base_dir: &Path) -> Vec<FootageId> {
    project.footage.iter().filter(|f| !resolve_path(base_dir, &f.path).exists()).map(|f| f.id.clone()).collect()
}

/// テキストレイヤーで指定されているが、この環境に無いフォントファミリー。
pub fn missing_fonts(project: &Project) -> Vec<String> {
    let mut all: Vec<String> = project.compositions.iter().flat_map(|c| rights::used_fonts(project, &c.id)).collect();
    all.sort();
    all.dedup();
    all.into_iter().filter(|f| !znd_text::font_available(f)).collect()
}

fn index_dir(dir: &Path, depth: usize, out: &mut HashMap<String, PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut subdirs = Vec::new();
    for e in rd.filter_map(|e| e.ok()) {
        let path = e.path();
        if path.is_dir() {
            // 隠しフォルダ（自動保存・プロキシなど）は探さない
            if depth < 6 && !e.file_name().to_string_lossy().starts_with('.') {
                subdirs.push(path);
            }
        } else {
            // 同名のファイルが複数あるときは、浅い階層のものを優先する
            out.entry(e.file_name().to_string_lossy().into_owned()).or_insert(path);
        }
    }
    for d in subdirs {
        index_dir(&d, depth + 1, out);
    }
}

/// 欠落しているフッテージを、`search_dir` 以下から同じファイル名で探してつなぎ直す。つないだ数を返す。
pub fn relink(project: &mut Project, base_dir: &Path, search_dir: &Path) -> usize {
    let missing = missing_footage(project, base_dir);
    if missing.is_empty() {
        return 0;
    }
    let mut index = HashMap::new();
    index_dir(search_dir, 0, &mut index);
    let mut n = 0;
    for item in project.footage.iter_mut().filter(|f| missing.contains(&f.id)) {
        let name = Path::new(&item.path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if let Some(found) = index.get(&name) {
            item.path = found.display().to_string();
            n += 1;
        }
    }
    n
}

#[derive(Debug, Clone, PartialEq)]
pub struct CollectReport {
    /// 収集先に書いたプロジェクトファイル。
    pub project_path: PathBuf,
    pub copied: usize,
    /// コピーできなかった（元ファイルが無い）素材の名前。
    pub missing: Vec<String>,
    pub total_bytes: u64,
}

fn or_blank(s: &str) -> &str {
    if s.is_empty() {
        "（未記入）"
    } else {
        s
    }
}

fn rights_table(project: &Project) -> String {
    let mut out = String::from("素材ごとの権利情報\n==================\n");
    for f in &project.footage {
        let r = &f.rights;
        let status = rights::issues(f);
        out += &format!(
            "\n■ {}\n  ファイル: {}\n  作者: {}\n  入手元: {}\n  ライセンス: {}\n  許諾日: {}\n  状態: {}\n  メモ: {}\n",
            f.name,
            f.path,
            or_blank(&r.author),
            or_blank(&r.source_url),
            or_blank(&r.license),
            or_blank(&r.cleared_on),
            if status.is_empty() { "すべて確認済み".to_string() } else { format!("要確認（{}）", status.join("、")) },
            r.notes
        );
    }
    out
}

/// プロジェクトと素材を 1 つのフォルダに集める（受け渡し・保管用）。
///
/// `comp` を指定すると、そのコンポで使っている素材だけを集める。素材は `footage/` にコピーし、
/// プロジェクト内のパスを相対パスに書き換える。権利情報とクレジット文面のテキストも添える。
pub fn collect_files(project: &Project, base_dir: &Path, comp: Option<&CompId>, dest: &Path) -> Result<CollectReport, MediaError> {
    let io = |p: &Path, e: std::io::Error| MediaError::File { path: p.display().to_string(), message: e.to_string() };
    let footage_dir = dest.join("footage");
    std::fs::create_dir_all(&footage_dir).map_err(|e| io(&footage_dir, e))?;
    let mut out = match comp {
        Some(id) => {
            znd_core::collab::export_comp(project, id).ok_or_else(|| MediaError::Invalid(format!("コンポ「{id}」が見つかりません")))?
        }
        None => project.clone(),
    };
    out.name = project.name.clone();
    let mut report = CollectReport { project_path: dest.join("project.znd.json"), copied: 0, missing: Vec::new(), total_bytes: 0 };
    let mut used_names: Vec<String> = Vec::new();
    for item in &mut out.footage {
        let src = resolve_path(base_dir, &item.path);
        let mut name = src.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| item.id.0.clone());
        // 別フォルダの同名ファイルがぶつからないよう、重複したら ID を前に付ける
        if used_names.contains(&name) {
            name = format!("{}_{name}", item.id);
        }
        used_names.push(name.clone());
        let target = footage_dir.join(&name);
        match std::fs::copy(&src, &target) {
            Ok(bytes) => {
                report.copied += 1;
                report.total_bytes += bytes;
                // OBJ のマテリアルなど、同じ名前で拡張子違いの付属ファイルも持っていく
                if let Ok(bytes) = std::fs::copy(src.with_extension("mtl"), target.with_extension("mtl")) {
                    report.total_bytes += bytes;
                }
                item.path = format!("footage/{name}");
            }
            Err(_) => report.missing.push(item.name.clone()),
        }
    }
    znd_core::io::save(&out, &report.project_path).map_err(|e| MediaError::Invalid(e.to_string()))?;
    let write = |name: &str, text: String| -> Result<(), MediaError> {
        let p = dest.join(name);
        std::fs::write(&p, text).map_err(|e| io(&p, e))
    };
    write("権利情報.txt", rights_table(&out))?;
    let credits: String = out.compositions.iter().map(|c| format!("【{}】\n{}\n", c.name, rights::credits(&out, &c.id))).collect();
    write("クレジット.txt", credits)?;
    Ok(report)
}
