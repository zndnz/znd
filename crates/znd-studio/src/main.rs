//! znd studio。実行は Rust、操作は TypeScript。
//!
//! Rust 側はプロジェクトの保持・取り消し履歴・評価・レンダリング・ファイルの読み書きだけを受け持つ。
//! 画面（`studio/`、TypeScript）はプロジェクトの JSON を受け取り、編集した JSON を `commit` で送り返す。
//! 送り返されたプロジェクトは検査してから採用し、取り消し履歴に積み、プレビューを作り直す。
//!
//! 画面へ渡すフレームは 8 バイトの見出し（幅・高さ、リトルエンディアン u32）＋ RGBA8 のバイト列。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod effects;
mod layers;
mod menu;
mod props;
mod sound;

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::ipc::Response;
use tauri::{AppHandle, Emitter, Manager, State};
use znd_core::demo::demo_project;
use znd_core::*;
use znd_media::{import_file, ImportOptions};
use znd_preview::PreviewEngine;
use sound::Sound;

/// 取り消し履歴の上限。
const UNDO_LIMIT: usize = 200;

struct Studio {
    project: Arc<Project>,
    revision: u64,
    path: Option<PathBuf>,
    dirty: bool,
    undo: Vec<(String, Arc<Project>)>,
    redo: Vec<(String, Arc<Project>)>,
    /// 直前の編集の名前（ドラッグ中の連続した編集を 1 つの取り消し単位にまとめる）。
    last_label: Option<String>,
    engine: Option<PreviewEngine>,
    sound: Arc<Sound>,
    /// 音を最後にミックスし直すか確かめた（リビジョン, コンポ）。
    sound_checked: (u64, String),
    app: Option<AppHandle>,
}

type Shared = Mutex<Studio>;

fn lock<'a>(s: &'a State<'_, Shared>) -> std::sync::MutexGuard<'a, Studio> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

fn base_dir(path: Option<&Path>) -> PathBuf {
    path.and_then(|p| p.parent()).filter(|p| !p.as_os_str().is_empty()).map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
}

impl Studio {
    fn new(project: Project, path: Option<PathBuf>) -> Self {
        Studio {
            project: Arc::new(project),
            revision: 1,
            path,
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            last_label: None,
            engine: None,
            sound: Sound::new(),
            sound_checked: (0, String::new()),
            app: None,
        }
    }

    /// 音を、いまの状態に合わせて（必要なら）ミックスし直す。
    fn sync_sound(&mut self, comp: &str) {
        if self.sound_checked.0 == self.revision && self.sound_checked.1 == comp {
            return;
        }
        let Some(engine) = &self.engine else { return };
        self.sound.sync(&self.project, &CompId::new(comp), &engine.clips);
        self.sound_checked = (self.revision, comp.to_string());
    }

    /// プレビューのエンジンを（素材の置き場所が変わったときは作り直して）用意する。
    fn start_engine(&mut self) {
        let Some(app) = self.app.clone() else { return };
        let engine = PreviewEngine::new(&base_dir(self.path.as_deref()), move || {
            let _ = app.emit("frame-ready", ());
        });
        self.engine = Some(engine);
        // 素材を読み直すので、音もミックスし直す
        self.sound.stop();
        self.sound.invalidate();
        self.sound_checked = (0, String::new());
    }

    fn replace(&mut self, project: Project, path: Option<PathBuf>) {
        self.project = Arc::new(project);
        self.path = path;
        self.dirty = false;
        self.undo.clear();
        self.redo.clear();
        self.last_label = None;
        self.revision += 1;
        self.start_engine();
    }

    /// 新しい状態を採用する。`coalesce` なら、同じ名前の直前の編集とまとめて 1 回の取り消しにする。
    fn apply(&mut self, project: Project, label: &str, coalesce: bool) {
        let merge = coalesce && self.last_label.as_deref() == Some(label) && !self.undo.is_empty();
        if !merge {
            self.undo.push((label.to_string(), self.project.clone()));
            if self.undo.len() > UNDO_LIMIT {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.last_label = Some(label.to_string());
        self.project = Arc::new(project);
        self.dirty = true;
        self.revision += 1;
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            project: self.project.clone(),
            revision: self.revision,
            path: self.path.as_ref().map(|p| p.display().to_string()),
            dirty: self.dirty,
            undo: self.undo.last().map(|u| u.0.clone()),
            redo: self.redo.last().map(|u| u.0.clone()),
            gpu: znd_preview::gpu_name().map(str::to_string),
        }
    }
}

/// 画面に渡す状態。
#[derive(Serialize)]
struct Snapshot {
    project: Arc<Project>,
    revision: u64,
    path: Option<String>,
    dirty: bool,
    /// 取り消せる編集の名前。
    undo: Option<String>,
    redo: Option<String>,
    gpu: Option<String>,
}

#[tauri::command]
fn snapshot(state: State<'_, Shared>) -> Snapshot {
    lock(&state).snapshot()
}

/// 画面で編集したプロジェクトを採用する。壊れた参照（循環する親子・プリコンポなど）があれば採用しない。
#[tauri::command]
fn commit(state: State<'_, Shared>, project: Project, label: String, coalesce: bool) -> Result<Snapshot, String> {
    let problems = project.validate();
    if !problems.is_empty() {
        return Err(problems.join("\n"));
    }
    let mut s = lock(&state);
    s.apply(project, &label, coalesce);
    Ok(s.snapshot())
}

#[tauri::command]
fn undo(state: State<'_, Shared>) -> Snapshot {
    let mut s = lock(&state);
    if let Some((label, p)) = s.undo.pop() {
        let cur = std::mem::replace(&mut s.project, p);
        s.redo.push((label, cur));
        s.revision += 1;
        s.dirty = true;
        s.last_label = None;
    }
    s.snapshot()
}

#[tauri::command]
fn redo(state: State<'_, Shared>) -> Snapshot {
    let mut s = lock(&state);
    if let Some((label, p)) = s.redo.pop() {
        let cur = std::mem::replace(&mut s.project, p);
        s.undo.push((label, cur));
        s.revision += 1;
        s.dirty = true;
        s.last_label = None;
    }
    s.snapshot()
}

#[tauri::command]
fn new_demo(state: State<'_, Shared>) -> Snapshot {
    let mut s = lock(&state);
    s.replace(demo_project(), None);
    s.snapshot()
}

/// プロジェクトを開く。`path` が無ければファイルを選ぶ画面を出す（選ばなければ何もしない）。
#[tauri::command]
fn open(state: State<'_, Shared>, path: Option<String>) -> Result<Option<Snapshot>, String> {
    let path = match path {
        Some(p) => PathBuf::from(p),
        None => match rfd::FileDialog::new().add_filter("znd プロジェクト", &["json"]).pick_file() {
            Some(p) => p,
            None => return Ok(None),
        },
    };
    let project = io::load(&path).map_err(|e| format!("開けません: {e}"))?;
    let mut s = lock(&state);
    s.replace(project, Some(path));
    Ok(Some(s.snapshot()))
}

/// 保存する。`choose` か保存先が未定なら、保存先を選ぶ画面を出す。
#[tauri::command]
fn save(state: State<'_, Shared>, choose: bool) -> Result<Option<Snapshot>, String> {
    let mut s = lock(&state);
    let path = match (&s.path, choose) {
        (Some(p), false) => p.clone(),
        _ => match rfd::FileDialog::new().add_filter("znd プロジェクト", &["json"]).set_file_name("project.znd.json").save_file() {
            Some(p) => p,
            None => return Ok(None),
        },
    };
    io::save(&s.project, &path).map_err(|e| format!("保存できません: {e}"))?;
    let moved = s.path.as_ref() != Some(&path);
    s.path = Some(path);
    s.dirty = false;
    if moved {
        // 素材の相対パスの基準が変わる
        s.start_engine();
    }
    Ok(Some(s.snapshot()))
}

/// 素材を読み込み、`comp` の `frame` の位置にレイヤーとして置く。
#[tauri::command]
fn import(state: State<'_, Shared>, comp: String, frame: i64, sequence: bool) -> Result<Option<Snapshot>, String> {
    let Some(path) = rfd::FileDialog::new().pick_file() else { return Ok(None) };
    let mut s = lock(&state);
    let comp_id = CompId::new(comp);
    let fr = s.project.comp(&comp_id).map(|c| c.frame_rate).ok_or("コンポがありません")?;
    let mut project = (*s.project).clone();
    let opts = ImportOptions { sequence, at: fr.frame_to_time(frame), ..Default::default() };
    import_file(&mut project, &comp_id, &path, opts).map_err(|e| format!("読み込めません: {e}"))?;
    s.apply(project, "素材の読み込み", false);
    Ok(Some(s.snapshot()))
}

/// 表示したいフレームを伝え、キャッシュにあればその画像を返す（無ければ空。できたら "frame-ready" を送る）。
#[tauri::command]
fn frame(state: State<'_, Shared>, comp: String, frame: i64, downsample: u32) -> Response {
    let mut s = lock(&state);
    // 表示しているコンポの音を先にミックスしておく（再生を押してすぐ鳴るように）
    s.sync_sound(&comp);
    let Some(engine) = &s.engine else { return Response::new(Vec::new()) };
    engine.request(&s.project, s.revision, &CompId::new(comp), frame, downsample.max(1));
    match engine.get(s.revision, frame, downsample.max(1)) {
        Some(p) => {
            let mut out = Vec::with_capacity(8 + p.rgba.len());
            out.extend((p.width as u32).to_le_bytes());
            out.extend((p.height as u32).to_le_bytes());
            out.extend(&p.rgba);
            Response::new(out)
        }
        None => Response::new(Vec::new()),
    }
}

fn frame_rate(s: &Studio, comp: &str) -> Option<FrameRate> {
    s.project.comp(&CompId::new(comp)).map(|c| c.frame_rate)
}

/// 再生を始める（音があれば鳴らす）。`start`〜`end` のフレームを繰り返す。
#[tauri::command]
fn play(state: State<'_, Shared>, comp: String, frame: i64, start: i64, end: i64) {
    let mut s = lock(&state);
    s.sync_sound(&comp);
    if let Some(fr) = frame_rate(&s, &comp) {
        s.sound.play(fr, frame, (start, end));
    }
}

#[tauri::command]
fn stop(state: State<'_, Shared>) {
    lock(&state).sound.stop();
}

/// 再生中、いま鳴っている位置のフレーム（音が無ければ null。画面は経過時間で進める）。
/// `frame` は画面が経過時間から求めたフレーム。
#[tauri::command]
fn play_clock(state: State<'_, Shared>, comp: String, frame: i64) -> Option<i64> {
    let mut s = lock(&state);
    // 再生中の編集にも追いつく
    s.sync_sound(&comp);
    let fr = frame_rate(&s, &comp)?;
    s.sound.clock(fr, frame)
}

/// RAM プレビュー済みのフレーム番号。
#[tauri::command]
fn cached(state: State<'_, Shared>, total: i64, downsample: u32) -> Vec<i64> {
    let s = lock(&state);
    s.engine.as_ref().map(|e| e.cached_frames(s.revision, total, downsample.max(1))).unwrap_or_default()
}

/// 新しいレイヤーの雛形を作る（画面がプロジェクトに挿して `commit` する）。
#[tauri::command]
fn make_layer(state: State<'_, Shared>, kind: String, comp: String, frame: i64) -> Result<Layer, String> {
    let s = lock(&state);
    layers::make(&s.project, &CompId::new(comp), &kind, frame)
}

/// レイヤーのトランスフォームの、その時刻の値（キーフレームを補間したもの）。
#[tauri::command]
fn eval_layer(state: State<'_, Shared>, comp: String, layer: String, frame: i64) -> Option<layers::Evaluated> {
    let s = lock(&state);
    let comp = s.project.comp(&CompId::new(comp))?;
    let layer = comp.layer(&LayerId::new(layer))?;
    Some(layers::evaluate(layer, comp.frame_rate.frame_to_time(frame)))
}

/// エフェクトの一覧。
#[tauri::command]
fn effect_catalog() -> Vec<effects::EffectInfo> {
    effects::catalog()
}

/// エフェクトを既定値で作る（大きさに依存する既定値は、レイヤーの大きさから決める）。
#[tauri::command]
fn make_effect(state: State<'_, Shared>, kind: String, comp: String, layer: String) -> Result<EffectInstance, String> {
    let s = lock(&state);
    let comp = s.project.comp(&CompId::new(comp)).ok_or("コンポがありません")?;
    let layer = comp.layer(&LayerId::new(layer)).ok_or("レイヤーがありません")?;
    let (w, h) = s.project.source_size(&layer.kind).unwrap_or((comp.width as f64, comp.height as f64));
    EffectInstance::new(&kind, w, h).ok_or_else(|| format!("エフェクト「{kind}」がありません"))
}

/// レイヤーの全プロパティと、時刻 `time` での値（画面が持っているレイヤーをそのまま渡す）。
#[tauri::command]
fn layer_props(layer: Layer, time: Time) -> Vec<props::PropInfo> {
    props::list(&layer, time)
}

/// プロパティへの操作（値の設定・キーフレーム・補間・エクスプレッション）。書き換えたレイヤーを返す。
#[tauri::command]
fn prop_edit(layer: Layer, path: String, time: Time, op: znd_core::props::PropEdit) -> Layer {
    props::edit(layer, &path, time, &op)
}

/// 選んだキーへの一括操作（イージングカーブ・補間・移動・削除）。
#[tauri::command]
fn keys_op(layer: Layer, keys: Vec<znd_core::keys::KeyAddr>, op: props::KeysOp) -> props::KeysResult {
    props::keys_op(layer, keys, &op)
}

#[tauri::command]
fn curve_of(layer: Layer, keys: Vec<znd_core::keys::KeyAddr>) -> Option<EaseCurve> {
    props::curve_of(&layer, &keys)
}

#[tauri::command]
fn curve_presets() -> Vec<props::Preset> {
    props::presets()
}

#[tauri::command]
fn curve_samples(curve: EaseCurve, n: usize) -> Vec<f64> {
    props::samples(&curve, n)
}

#[tauri::command]
fn parse_curve(text: String) -> Option<EaseCurve> {
    props::parse_curve(&text)
}

/// インストール済みのフォント（ファミリー名と、日本語名などの別名）。
#[tauri::command]
fn fonts() -> Vec<(String, Vec<String>)> {
    znd_text::font_families_with_aliases()
}

fn main() {
    // 使い方: znd-studio [プロジェクト.znd.json]
    let arg = std::env::args().nth(1).map(PathBuf::from);
    let studio = match arg.as_ref().map(|p| (p, io::load(p))) {
        Some((p, Ok(project))) => Studio::new(project, Some(p.clone())),
        Some((p, Err(e))) => {
            eprintln!("{} を開けません: {e}", p.display());
            Studio::new(demo_project(), None)
        }
        None => Studio::new(demo_project(), None),
    };
    tauri::Builder::default()
        .manage(Mutex::new(studio))
        .setup(|app| {
            let state = app.state::<Shared>();
            let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
            s.app = Some(app.handle().clone());
            s.start_engine();
            drop(s);
            app.set_menu(menu::build(app.handle())?)?;
            Ok(())
        })
        .on_menu_event(|app, event| menu::forward(app, event.id().as_ref()))
        .invoke_handler(tauri::generate_handler![
            snapshot,
            commit,
            undo,
            redo,
            new_demo,
            open,
            save,
            import,
            frame,
            play,
            stop,
            play_clock,
            cached,
            make_layer,
            eval_layer,
            effect_catalog,
            make_effect,
            layer_props,
            prop_edit,
            keys_op,
            curve_of,
            curve_presets,
            curve_samples,
            parse_curve,
            fonts
        ])
        .run(tauri::generate_context!())
        .expect("znd studio を起動できません");
}
