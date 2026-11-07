//! プロジェクトのファイルまわりと履歴: 歌詞・カメラ・コンポの受け渡しの読み込み、素材の再リンク・収集、
//! 自動保存と復旧、制作ログ、取り消し履歴の一覧と移動、版（スナップショット）、レビューコメント。

use crate::{base_dir, lock, unix_now, Shared, Snapshot, Studio};
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::ipc::Response;
use tauri::State;
use znd_core::*;

/// 自動保存の間隔と、残す世代。
pub const AUTOSAVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(120);
pub const AUTOSAVE_GENERATIONS: usize = 20;

/// 操作の結果。プロジェクトが変わったときだけ `snapshot` がある。
#[derive(Serialize)]
pub struct Reply {
    pub snapshot: Option<Snapshot>,
    pub status: String,
}

impl Studio {
    /// プロジェクトを複製して `f` で書き換え、検査してから採用する。`f` が Err なら何も変えない。
    pub(crate) fn mutate(&mut self, label: &str, f: impl FnOnce(&mut Project) -> Result<String, String>) -> Reply {
        let mut p = (*self.project).clone();
        match f(&mut p) {
            Ok(status) => {
                let problems = p.validate();
                if !problems.is_empty() {
                    return Reply { snapshot: None, status: format!("{label}できません: {}", problems.join(" / ")) };
                }
                self.apply(p, label, false);
                Reply { snapshot: Some(self.snapshot()), status }
            }
            Err(e) => Reply { snapshot: None, status: e },
        }
    }

    pub(crate) fn reply(&self, status: impl Into<String>) -> Reply {
        Reply { snapshot: None, status: status.into() }
    }

    /// 自動保存の基準パス。未保存プロジェクトは一時ディレクトリに退避する。
    pub(crate) fn autosave_base(&self) -> PathBuf {
        self.path.clone().unwrap_or_else(|| std::env::temp_dir().join("znd-untitled.znd.json"))
    }

    pub(crate) fn autosave_now(&mut self) -> Result<PathBuf, String> {
        let r = io::autosave(&self.project, &self.autosave_base(), AUTOSAVE_GENERATIONS).map_err(|e| format!("自動保存に失敗しました: {e}"));
        if r.is_ok() {
            self.unsaved_since_autosave = false;
        }
        r
    }

    /// 終了するとき: 保存せずに閉じても次回に復旧できるよう自動保存し、制作ログに今回の作業を記録する。
    pub(crate) fn on_exit(&mut self) {
        if self.dirty {
            let _ = self.autosave_now();
        }
        if let Some(path) = &self.path {
            if self.session.edits > 0 {
                let entry = log::SessionEntry {
                    started: self.session.started,
                    seconds: self.session.active.as_secs(),
                    edits: self.session.edits,
                    user: self.user.clone(),
                };
                let _ = log::append_session(path, &entry);
                self.session.edits = 0;
            }
        }
    }

    fn comp_index(&self, comp: &str) -> Result<usize, String> {
        self.project.compositions.iter().position(|c| c.id.0 == comp).ok_or_else(|| "コンポがありません".into())
    }
}

/// 制作ログのための、作業していた時間の数え方（操作の間が 2 分を超えたら休んでいたとみなす）。
pub struct Session {
    pub started: u64,
    pub active: std::time::Duration,
    pub last: std::time::Instant,
    pub edits: u32,
}

const IDLE_LIMIT: std::time::Duration = std::time::Duration::from_secs(120);

impl Session {
    pub fn new() -> Self {
        Session { started: unix_now(), active: Default::default(), last: std::time::Instant::now(), edits: 0 }
    }

    pub fn touch(&mut self) {
        let now = std::time::Instant::now();
        let gap = now.duration_since(self.last);
        if gap < IDLE_LIMIT {
            self.active += gap;
        }
        self.last = now;
    }
}

/// プロジェクトを開いたときの注意（構造の問題・欠落した素材やフォント）。
pub fn open_notes(project: &Project, path: &Path) -> String {
    let issues = project.validate().len();
    let missing = znd_media::manage::missing_footage(project, &base_dir(Some(path))).len();
    let fonts = znd_media::manage::missing_fonts(project);
    let mut notes = Vec::new();
    if issues > 0 {
        notes.push(format!("構造の問題 {issues} 件"));
    }
    if missing > 0 {
        notes.push(format!("欠落した素材 {missing} 個（ファイル → 欠落した素材を再リンク）"));
    }
    if !fonts.is_empty() {
        notes.push(format!("欠落フォント: {}", fonts.join("、")));
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if notes.is_empty() {
        format!("{name} を開きました")
    } else {
        format!("{name} を開きました — {}", notes.join(" / "))
    }
}

// ---------- 読み込み ----------

#[tauri::command]
pub fn import_lyrics(state: State<'_, Shared>, comp: String) -> Option<Reply> {
    let path = rfd::FileDialog::new().add_filter("歌詞・字幕", &["lrc", "srt", "ass", "ssa"]).pick_file()?;
    let mut s = lock(&state);
    let i = match s.comp_index(&comp) {
        Ok(i) => i,
        Err(e) => return Some(s.reply(e)),
    };
    Some(s.mutate("歌詞の読み込み", |p| {
        let text = std::fs::read(&path).map_err(|e| format!("歌詞を読み込めません: {e}"))?;
        let lines = lyrics::parse_lyrics(&path.to_string_lossy(), &crate::plugins::decode_text(&text)).map_err(|e| format!("歌詞を読み込めません: {e}"))?;
        let n = lyrics::add_lyric_layers(p, i, &lines, 72.0);
        Ok(format!("歌詞 {n} 行をテキストレイヤーとマーカーにしました"))
    }))
}

#[tauri::command]
pub fn import_camera(state: State<'_, Shared>, comp: String) -> Option<Reply> {
    let path = rfd::FileDialog::new().add_filter("カメラデータ", &["chan", "abc"]).pick_file()?;
    let mut s = lock(&state);
    let id = CompId::new(comp.as_str());
    let Some(c) = s.project.comp(&id).cloned() else { return Some(s.reply("コンポがありません")) };
    let parsed = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("abc")) {
        znd_media::abc_camera_layer(&path, c.width, c.height, c.frame_rate, 100.0).map_err(|e| e.to_string())
    } else {
        std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| camera_import::parse_chan(&t, c.width, c.height, c.frame_rate, 100.0, 39.6))
    };
    let fresh = s.project.fresh_id("camera");
    Some(s.mutate("カメラの読み込み", |p| {
        let mut cam = parsed.map_err(|e| format!("カメラを読み込めません: {e}"))?;
        cam.id = LayerId::new(fresh);
        cam.out_point = cam.out_point.max(c.duration);
        p.comp_mut(&id).ok_or("コンポがありません")?.layers.insert(0, cam);
        Ok("カメラを読み込みました（1 単位 = 100px）".into())
    }))
}

/// 欠落した素材を、選んだフォルダの中から探してつなぎ直す。
#[tauri::command]
pub fn relink_footage(state: State<'_, Shared>) -> Option<Reply> {
    let dir = rfd::FileDialog::new().pick_folder()?;
    let mut s = lock(&state);
    let base = base_dir(s.path.as_deref());
    let mut reply = s.mutate("素材の再リンク", |p| {
        let n = znd_media::manage::relink(p, &base, &dir);
        let left = znd_media::manage::missing_footage(p, &base).len();
        if n == 0 {
            return Err(format!("つなぎ直せる素材は見つかりませんでした（欠落 {left} 個）"));
        }
        Ok(format!("{n} 個をつなぎ直しました（残り {left} 個）"))
    });
    if reply.snapshot.is_some() {
        // 素材を読み直す
        s.start_engine();
        reply.snapshot = Some(s.snapshot());
    }
    Some(reply)
}

/// 使っている素材とプロジェクトを、選んだフォルダに集める（権利情報付き）。
#[tauri::command]
pub fn collect_files(state: State<'_, Shared>) -> Option<String> {
    let dir = rfd::FileDialog::new().pick_folder()?;
    let s = lock(&state);
    Some(match znd_media::manage::collect_files(&s.project, &base_dir(s.path.as_deref()), None, &dir) {
        Ok(r) => format!(
            "{} 個の素材を {} に集めました{}",
            r.copied,
            dir.display(),
            if r.missing.is_empty() { String::new() } else { format!("（見つからない素材 {} 個）", r.missing.len()) }
        ),
        Err(e) => format!("収集に失敗しました: {e}"),
    })
}

/// コンポ（と、その中で使うコンポ・素材）だけを別のファイルに書き出す（受け渡し用）。
#[tauri::command]
pub fn export_comp_package(state: State<'_, Shared>, comp: String) -> Option<String> {
    let s = lock(&state);
    let Some(pkg) = collab::export_comp(&s.project, &CompId::new(comp.as_str())) else { return Some("コンポがありません".into()) };
    let name = s.project.comp(&CompId::new(comp.as_str())).map(|c| c.name.clone()).unwrap_or_else(|| "comp".into());
    drop(s);
    let path = rfd::FileDialog::new().add_filter("znd プロジェクト", &["json"]).set_file_name(format!("{name}.znd.json")).save_file()?;
    Some(match io::save(&pkg, &path) {
        Ok(()) => format!("コンポを {} に書き出しました", path.display()),
        Err(e) => format!("書き出せません: {e}"),
    })
}

#[tauri::command]
pub fn import_comp_package(state: State<'_, Shared>) -> Option<Reply> {
    let path = rfd::FileDialog::new().add_filter("znd プロジェクト", &["json"]).pick_file()?;
    let mut s = lock(&state);
    Some(s.mutate("コンポの読み込み", |p| {
        let pkg = io::load(&path).map_err(|e| format!("読み込めません: {e}"))?;
        let id = collab::import_comp(p, &pkg).ok_or("コンポがありません")?;
        Ok(format!("コンポ「{id}」を読み込みました"))
    }))
}

/// 表示中のフレーム（プレビューの解像度）を PNG で保存する。
#[tauri::command]
pub fn save_frame_image(state: State<'_, Shared>, frame: i64, downsample: u32) -> Option<String> {
    let preview = {
        let s = lock(&state);
        let engine = s.engine.as_ref()?;
        engine.get(s.revision, frame, downsample.max(1))
    };
    let Some(p) = preview else { return Some("このフレームはまだレンダリング中です".into()) };
    let path = rfd::FileDialog::new().add_filter("PNG", &["png"]).set_file_name(format!("frame_{frame:05}.png")).save_file()?;
    Some(match image::save_buffer(&path, &p.rgba, p.width as u32, p.height as u32, image::ExtendedColorType::Rgba8) {
        Ok(()) => format!("{} に保存しました", path.display()),
        Err(e) => format!("保存できません: {e}"),
    })
}

// ---------- 自動保存からの復旧 ----------

/// 保存済みのファイルより新しい自動保存を復元する（`restore` が false なら、次回は聞かない）。
#[tauri::command]
pub fn recover(state: State<'_, Shared>, restore: bool) -> Reply {
    let mut s = lock(&state);
    let Some(auto) = s.recovery.take() else { return s.reply("") };
    if !restore {
        // 未保存プロジェクトの退避分は、無視を選んだら次回また聞かないよう消す
        if s.path.is_none() {
            for old in io::list_autosaves(&s.autosave_base()) {
                let _ = std::fs::remove_file(old);
            }
        }
        return Reply { snapshot: Some(s.snapshot()), status: String::new() };
    }
    let reply = s.mutate("自動保存を復元", |p| {
        *p = io::load(&auto).map_err(|e| format!("復元できません: {e}"))?;
        Ok("自動保存を復元しました（⌘Z で元に戻せます）".into())
    });
    Reply { snapshot: Some(s.snapshot()), status: reply.status }
}

// ---------- 取り消し履歴 ----------

#[derive(Serialize)]
pub struct History {
    /// 古い順。
    undo: Vec<String>,
    /// 次にやり直すものから順に。
    redo: Vec<String>,
}

#[tauri::command]
pub fn history(state: State<'_, Shared>) -> History {
    let s = lock(&state);
    History { undo: s.undo.iter().map(|u| u.0.clone()).collect(), redo: s.redo.iter().rev().map(|u| u.0.clone()).collect() }
}

/// `undo` が `index` 個になるまで取り消す、または `redo` を `forward` 個やり直す。
#[tauri::command]
pub fn history_jump(state: State<'_, Shared>, index: Option<usize>, forward: Option<usize>) -> Snapshot {
    let mut s = lock(&state);
    if let Some(i) = index {
        while s.undo.len() > i {
            s.step_undo();
        }
    }
    for _ in 0..forward.unwrap_or(0) {
        s.step_redo();
    }
    s.snapshot()
}

// ---------- 版（スナップショット） ----------

#[derive(Serialize)]
pub struct Version {
    name: String,
    /// 保存した時刻（UNIX 秒）。
    saved: u64,
    file: String,
}

#[tauri::command]
pub fn versions(state: State<'_, Shared>) -> Option<Vec<Version>> {
    let s = lock(&state);
    let path = s.path.clone()?;
    Some(
        io::list_snapshots(&path)
            .into_iter()
            .rev()
            .map(|(name, saved, file)| Version { name, saved, file: file.display().to_string() })
            .collect(),
    )
}

#[tauri::command]
pub fn version_save(state: State<'_, Shared>, name: String) -> String {
    let s = lock(&state);
    let Some(path) = s.path.clone() else { return "版を残すには、先にプロジェクトを保存してください".into() };
    match io::save_snapshot(&s.project, &path, &name) {
        Ok(_) => format!("版「{name}」を保存しました"),
        Err(e) => format!("保存できません: {e}"),
    }
}

#[tauri::command]
pub fn version_restore(state: State<'_, Shared>, file: String, name: String) -> Reply {
    let mut s = lock(&state);
    s.mutate(&format!("版「{name}」に戻す"), |p| {
        *p = io::load(Path::new(&file)).map_err(|e| format!("読み込めません: {e}"))?;
        Ok(format!("版「{name}」に戻しました（⌘Z で取り消せます）"))
    })
}

/// いまのフレームで、版との違いを赤く示した画像を作る（`diff_image` で受け取る）。説明の文を返す。
#[tauri::command]
pub fn version_compare(state: State<'_, Shared>, file: String, name: String, comp: String, frame: i64) -> Result<String, String> {
    let mut s = lock(&state);
    let id = CompId::new(comp.as_str());
    let t = s.project.comp(&id).map(|c| c.frame_rate.frame_to_time(frame)).ok_or("コンポがありません")?;
    let engine = s.engine.as_ref().ok_or("プレビューを準備中です")?;
    let old = io::load(Path::new(&file)).map_err(|e| format!("読み込めません: {e}"))?;
    let d = znd_media::review::version_diff(&old, &s.project, &*engine.footage, &id, t, 2).map_err(|e| format!("比較できません: {e}"))?;
    let rgba = znd_render::to_rgba8(&d.heatmap, None);
    let label = match d.bbox {
        Some(_) => format!("版「{name}」との違い: {} 画素（赤い部分）", d.changed_pixels),
        None => format!("版「{name}」との違いはありません（このフレーム）"),
    };
    s.diff = Some((d.heatmap.width, d.heatmap.height, rgba));
    Ok(label)
}

/// 比べた画像（8 バイトの見出し＋ RGBA8）。無ければ空。
#[tauri::command]
pub fn diff_image(state: State<'_, Shared>) -> Response {
    let s = lock(&state);
    match &s.diff {
        Some((w, h, rgba)) => {
            let mut out = Vec::with_capacity(8 + rgba.len());
            out.extend((*w as u32).to_le_bytes());
            out.extend((*h as u32).to_le_bytes());
            out.extend(rgba);
            Response::new(out)
        }
        None => Response::new(Vec::new()),
    }
}

#[tauri::command]
pub fn diff_close(state: State<'_, Shared>) {
    lock(&state).diff = None;
}

// ---------- レビューコメント ----------

/// コメントを付ける。ロック中のコンポにも付けられる（レビューする側は編集者とは別の人）。
#[tauri::command]
pub fn review_add(state: State<'_, Shared>, comp: String, frame: i64, text: String, point: Option<[f64; 2]>) -> Reply {
    let mut s = lock(&state);
    let id = CompId::new(comp.as_str());
    let Some(t) = s.project.comp(&id).map(|c| c.frame_rate.frame_to_time(frame)) else { return s.reply("コンポがありません") };
    let user = s.user.clone();
    s.mutate("レビューコメント", |p| {
        collab::add_review(p, &id, t, &user, text.trim(), point, &unix_now().to_string());
        Ok("コメントを追加しました".into())
    })
}

// ---------- 情報 ----------

#[tauri::command]
pub fn credits_text(state: State<'_, Shared>, comp: String) -> String {
    rights::credits(&lock(&state).project, &CompId::new(comp))
}

#[tauri::command]
pub fn production_log(state: State<'_, Shared>) -> String {
    let s = lock(&state);
    let sessions = s.path.as_ref().map(|p| log::read_sessions(p)).unwrap_or_default();
    log::summary_text(&s.project, &sessions)
}

#[derive(Serialize)]
pub struct License {
    name: &'static str,
    license: &'static str,
    usage: &'static str,
}

#[derive(Serialize)]
pub struct Licenses {
    components: Vec<License>,
    notice: &'static str,
}

#[tauri::command]
pub fn licenses() -> Licenses {
    Licenses {
        components: licenses::COMPONENTS.iter().map(|c| License { name: c.name, license: c.license, usage: c.usage }).collect(),
        notice: licenses::NOTICE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn studio() -> Studio {
        let mut p = Project::new("t");
        p.compositions.push(Composition::new("c", "c", 64, 36, FrameRate::FPS_30, Time::from_seconds(2.0)));
        Studio::new(p, None)
    }

    #[test]
    fn mutate_checks_the_result_and_records_history() {
        let mut s = studio();
        let r = s.mutate("名前", |p| {
            p.compositions[0].name = "x".into();
            Ok("ok".into())
        });
        assert!(r.snapshot.is_some());
        assert_eq!(s.undo.len(), 1);
        // 失敗したら何も変えない
        let r = s.mutate("失敗", |_| Err("だめ".into()));
        assert!(r.snapshot.is_none());
        assert_eq!(r.status, "だめ");
        assert_eq!(s.undo.len(), 1);
        // 壊れた参照は採用しない
        let r = s.mutate("壊す", |p| {
            let mut l = Layer::new("l", "l", LayerKind::Null, Transform::centered(0.0, 0.0, 1.0, 1.0), Time::from_seconds(1.0));
            l.parent = Some(LayerId::new("l"));
            p.compositions[0].layers.push(l);
            Ok("ok".into())
        });
        assert!(r.snapshot.is_none());
        assert_eq!(s.project.compositions[0].layers.len(), 0);
    }

    #[test]
    fn history_can_jump_back_and_forward() {
        let mut s = studio();
        for i in 0..3 {
            s.mutate(&format!("e{i}"), |p| {
                p.compositions[0].name = format!("{i}");
                Ok(String::new())
            });
        }
        while s.undo.len() > 1 {
            s.step_undo();
        }
        assert_eq!(s.project.compositions[0].name, "0");
        assert_eq!(s.redo.len(), 2);
        s.step_redo();
        assert_eq!(s.project.compositions[0].name, "1");
    }

    #[test]
    fn session_counts_only_active_time() {
        let mut ss = Session::new();
        ss.last -= std::time::Duration::from_secs(10);
        ss.touch();
        assert!(ss.active.as_secs() >= 10);
        ss.last -= std::time::Duration::from_secs(600);
        let before = ss.active;
        ss.touch();
        assert_eq!(ss.active, before);
    }
}
