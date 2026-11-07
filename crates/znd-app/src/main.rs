//! znd アプリ。ビューア・タイムライン・プロパティ編集・音声同期・書き出し・提出前チェック。

mod audio;
mod beatfit_ui;
mod easing;
mod font_preview;
mod icons;
mod keymap;
mod keyops;
mod panels;
mod plugin_host;
mod prefs;
mod preview;
mod rescue;
mod theme;
mod timeline;
mod tools;
mod uiscript;
mod widgets;
mod windows;
mod workspace;

use eframe::egui;
use preview::PreviewEngine;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use znd_audio::{time_to_sample, Mixer, SAMPLE_RATE};
use znd_core::demo::demo_project;
use znd_core::*;
use znd_media::{import_file, ExportReport, ImportOptions, QueueJob};
use znd_render::safety::FlashWarning;
use znd_script::ExprError;

const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(120);
const AUTOSAVE_GENERATIONS: usize = 20;
/// 同じプロパティへの連続した値編集（ドラッグ）を 1 回の取り消し単位にまとめる時間幅。
const COALESCE_WINDOW: Duration = Duration::from_millis(800);
/// これ以上操作が無ければ「作業していない」とみなす（制作ログの作業時間に数えない）。
const IDLE_LIMIT: Duration = Duration::from_secs(120);

fn main() -> eframe::Result<()> {
    rescue::install_panic_hook();
    // 使い方: znd-app [プロジェクト.znd.json] [--frame N] [--select レイヤーID] [--show ウィンドウ名] [--screenshot 出力.png]
    // （--select / --show / --screenshot は、画面の自動確認用）
    let mut project_path = None;
    let mut screenshot = None;
    let mut start_frame = 0;
    let mut select = None;
    let mut show = Vec::new();
    // 画面操作の自動確認用: ポインタやキーの操作を書いたファイルを再生する（uiscript.rs）
    let mut ui_script = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--screenshot" => screenshot = args.next().map(PathBuf::from),
            "--frame" => start_frame = args.next().and_then(|s| s.parse().ok()).unwrap_or(0),
            "--select" => select = args.next(),
            "--show" => show.extend(args.next()),
            "--ui-script" => ui_script = args.next().map(PathBuf::from),
            _ => project_path = Some(PathBuf::from(arg)),
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1720.0, 1000.0]).with_title("znd"),
        ..Default::default()
    };
    eframe::run_native(
        "znd",
        options,
        Box::new(move |cc| {
            install_japanese_font(&cc.egui_ctx);
            theme::apply(&cc.egui_ctx);
            let mut app = App::new(&cc.egui_ctx, project_path, start_frame, screenshot);
            app.selected = select.map(LayerId::new);
            app.script = ui_script.as_deref().and_then(uiscript::Script::load);
            for w in &show {
                match w.as_str() {
                    "export" => app.show.export = true,
                    "check" => app.show.check = true,
                    "history" => app.show.history = true,
                    "snapshots" => app.show.snapshots = true,
                    "reviews" => app.show.reviews = true,
                    "graph" => {
                        app.show.graph = true;
                        app.graph_path = Some("transform.position".into());
                    }
                    "script" => app.show.script = true,
                    "plugins" => app.show.plugins = true,
                    "shortcuts" => app.show.shortcuts = true,
                    "beatfit" => app.show.beatfit = true,
                    "screening" => app.screening = true,
                    _ => {}
                }
            }
            Ok(Box::new(app))
        }),
    )
}

/// egui 同梱フォントには日本語が無いので、OS のフォントを先頭に差し込む。
fn info_label(action: keymap::Action) -> &'static str {
    keymap::info(action).label
}

fn install_japanese_font(ctx: &egui::Context) {
    const CANDIDATES: [&str; 7] = [
        "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "C:\\Windows\\Fonts\\YuGothR.ttc",
        "C:\\Windows\\Fonts\\meiryo.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    ];
    let Some(bytes) = CANDIDATES.iter().find_map(|p| std::fs::read(p).ok()) else {
        eprintln!("日本語フォントが見つかりません。UI の日本語が表示されない可能性があります");
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("jp".into(), egui::FontData::from_owned(bytes));
    fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "jp".into());
    fonts.families.entry(egui::FontFamily::Monospace).or_default().push("jp".into());
    ctx.set_fonts(fonts);
}

/// エフェクトを GPU で処理するか（表示メニューで切り替える）。
static GPU_ENABLED: AtomicBool = AtomicBool::new(true);

fn gpu_backend() -> Option<&'static znd_gpu::GpuBackend> {
    static GPU: std::sync::OnceLock<Option<znd_gpu::GpuBackend>> = std::sync::OnceLock::new();
    GPU.get_or_init(|| {
        // 環境変数 ZND_GPU=0 で、GPU を一切初期化しない
        if std::env::var("ZND_GPU").is_ok_and(|v| v == "0") {
            return None;
        }
        match znd_gpu::GpuBackend::new() {
            Ok(g) => Some(g),
            Err(e) => {
                eprintln!("GPU を使えません（CPU で処理します）: {e}");
                None
            }
        }
    })
    .as_ref()
}

/// エフェクトの高速化バックエンド。GPU が無い・無効にしている場合は `None`（すべて CPU）。
pub(crate) fn gpu() -> Option<&'static dyn znd_render::effects::EffectAccel> {
    if !GPU_ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    gpu_backend().map(|g| g as &dyn znd_render::effects::EffectAccel)
}

/// 合成の GPU 化（エフェクトの GPU 処理と同じスイッチで切り替わる）。
pub(crate) fn gpu_composite() -> Option<&'static dyn znd_render::gpu_composite::CompositeAccel> {
    if !GPU_ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    gpu_backend().map(|g| g as &dyn znd_render::gpu_composite::CompositeAccel)
}

pub(crate) fn gpu_name() -> Option<&'static str> {
    gpu_backend().map(|g| g.name())
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// バックグラウンドで動く処理の進捗と結果。
pub(crate) struct Task<T> {
    pub label: String,
    pub done: Arc<AtomicI64>,
    pub total: Arc<AtomicI64>,
    pub cancel: Arc<AtomicBool>,
    pub result: Arc<Mutex<Option<Result<T, String>>>>,
}

impl<T: Send + 'static> Task<T> {
    /// `f(進捗を報告する関数)` を別スレッドで走らせる。報告関数が false を返したら中断すること。
    pub fn spawn(
        label: &str,
        ctx: &egui::Context,
        f: impl FnOnce(&mut dyn FnMut(i64, i64) -> bool) -> Result<T, String> + Send + 'static,
    ) -> Self {
        let task =
            Task { label: label.into(), done: Arc::default(), total: Arc::default(), cancel: Arc::default(), result: Arc::default() };
        let (done, total, cancel, result, ctx) =
            (task.done.clone(), task.total.clone(), task.cancel.clone(), task.result.clone(), ctx.clone());
        std::thread::spawn(move || {
            let mut report = |d: i64, t: i64| {
                done.store(d, Ordering::Relaxed);
                total.store(t, Ordering::Relaxed);
                ctx.request_repaint();
                !cancel.load(Ordering::Relaxed)
            };
            // 書き出しなどが内部エラーでパニックしても、作業中のアプリは止めず、失敗として伝える
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut report)))
                .unwrap_or_else(|e| Err(format!("内部エラーで中断しました: {}", preview::panic_text(&*e))));
            *result.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
            ctx.request_repaint();
        });
        task
    }

    pub fn take(&self) -> Option<Result<T, String>> {
        self.result.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    pub fn fraction(&self) -> f32 {
        self.done.load(Ordering::Relaxed) as f32 / self.total.load(Ordering::Relaxed).max(1) as f32
    }
}

struct Screenshot {
    path: PathBuf,
    started: Instant,
    requested: bool,
}

/// 提出前チェックの結果。
#[derive(Default)]
pub(crate) struct CheckResult {
    /// (問題なしか, 文面)。
    pub lines: Vec<(bool, String)>,
    pub flashes: Vec<FlashWarning>,
}

pub struct App {
    project: Project,
    /// `project` のスナップショット。UI の読み取りとレンダースレッドが共有する。
    shared: Arc<Project>,
    revision: u64,
    /// フォントの選択欄に出す見本の画像。
    pub(crate) font_previews: font_preview::FontPreviews,
    path: Option<PathBuf>,
    dirty: bool,
    unsaved_since_autosave: bool,
    last_autosave: Instant,
    /// (操作名, 操作前の状態)。
    undo: Vec<(String, Project)>,
    redo: Vec<(String, Project)>,
    last_edit: Option<(String, Instant)>,
    user: String,

    comp_id: CompId,
    frame: i64,
    selected: Option<LayerId>,
    selected_footage: Option<FootageId>,
    downsample: u32,
    /// 再生開始の（実時刻, フレーム）。
    pub(crate) playing: Option<(Instant, i64)>,
    /// ワークエリア [開始, 終了) フレーム。ループ再生と書き出し範囲に使う。
    work_area: Option<(i64, i64)>,
    snap: bool,
    hide_shy: bool,
    only_animated: bool,

    engine: PreviewEngine,
    texture: Option<egui::TextureHandle>,
    overlay: Option<egui::TextureHandle>,
    /// いまテクスチャに載っている（リビジョン, フレーム, ダウンサンプル）。
    shown: Option<(u64, i64, u32)>,
    screening: bool,
    zoom: f32,

    player: Option<audio::Player>,
    /// ミックス済みのコンポ音声（48kHz ステレオ）と、その元になった設定の署名。
    audio_buf: Arc<Vec<f32>>,
    audio_sig: u64,
    audio_task: Option<Task<(u64, Vec<f32>)>>,
    spectrogram: Option<egui::TextureHandle>,
    show_spectrogram: bool,
    last_scrub_frame: i64,

    status: String,
    /// エクスプレッションのエラー（リビジョンごとに検査し直す）。
    expr_errors: (u64, Vec<ExprError>),
    /// エクスプレッションを編集中のプロパティ（パス）。
    expr_open: Option<String>,
    expr_buf: String,
    graph_path: Option<String>,

    export_ui: windows::ExportUi,
    export_task: Option<Task<ExportReport>>,
    queue: Vec<QueueJob>,
    check: CheckResult,
    check_task: Option<Task<CheckResult>>,
    diff: Option<(egui::TextureHandle, String)>,
    snapshot_name: String,
    review_text: String,
    script_text: String,

    /// ビューアのツールと、ドラッグ中の操作。
    pub(crate) tool: tools::Tool,
    pub(crate) viewer_drag: Option<tools::ViewerDrag>,
    /// ペンツールで描いている途中のパス（レイヤー座標）。
    pub(crate) pen: Option<edit::PenPath>,
    /// 編集中のマスクと、選択中の頂点。
    pub(crate) active_mask: usize,
    pub(crate) active_vertex: Option<usize>,
    /// タイムラインの拡大率（1 = コンポ全体）と、左端のフレーム。
    pub(crate) tl_zoom: f64,
    pub(crate) tl_scroll: f64,
    pub(crate) tl_drag: Option<timeline::TlDrag>,
    /// 直前のフレームで描いたタイムラインの表示範囲。
    pub(crate) tl_view: Option<timeline::TlView>,
    /// タイムラインの左ペイン（レイヤーの列）の幅。
    pub(crate) tl_left: f32,
    /// タイムラインにモード・マット・親の列を出す。
    pub(crate) tl_detail: bool,
    /// タイムラインで開閉を既定から切り替えた行（レイヤー ID、または `レイヤー ID/グループ`）。
    pub(crate) tl_open: std::collections::HashSet<String>,
    pub(crate) tl_search: String,
    /// タイムラインで選択中のキーフレーム（複数可。レイヤーをまたいでもよい）。
    pub(crate) selected_keys: Vec<timeline::KeySel>,
    /// 矩形選択の途中で、今のフレームに行が拾ったキー。
    pub(crate) marquee_hits: Vec<timeline::KeySel>,
    /// 「ビートに合わせる」ウィンドウの状態。
    pub(crate) beat_ui: beatfit_ui::BeatUi,
    /// 利用者ごとの設定。
    pub(crate) prefs: prefs::Prefs,
    /// イージングのパネルの状態。
    pub(crate) ease: easing::EaseUi,
    /// グラフエディターに表示している時間の範囲（秒）。終わりが始まり以下なら全体。
    pub(crate) graph_view: (f64, f64),
    /// `graph_view` を決めたときのレイヤーとプロパティ。
    pub(crate) graph_view_for: Option<String>,
    /// キーボードショートカットの割り当て。
    pub(crate) keymap: keymap::Keymap,
    /// 割り当てを変更中の操作（次に押したキーを割り当てる）。
    pub(crate) keymap_capture: Option<keymap::Action>,
    /// コピーしたキーフレーム。
    pub(crate) key_clip: Option<znd_core::keys::KeyClipboard>,
    /// 最後にクリックしたプロパティの行（キーの貼り付け先）。
    pub(crate) key_target: Option<(LayerId, String)>,
    /// 各パネルで開いているタブ。
    pub(crate) right_tab: usize,
    pub(crate) project_search: String,
    /// ビューアに出すチャンネル（0 = RGB、1〜3 = R・G・B、4 = 輝度）。
    pub(crate) channel: u8,
    pub(crate) show_guides: bool,
    pub(crate) show_grid: bool,
    /// ビューア上のポインタの（コンポ座標, その画素の RGBA）。情報パネルに出す。
    pub(crate) hover_info: Option<([f64; 2], [u8; 4])>,
    /// 画面上の主要な領域の位置（操作スクリプトが座標を求めるのに使う）。
    pub(crate) anchors: std::collections::HashMap<String, egui::Rect>,
    pub(crate) script: Option<uiscript::Script>,
    pub(crate) script_shot: Option<PathBuf>,
    plugin_report: Vec<(PathBuf, String)>,
    pub(crate) plugins: plugin_host::PluginHost,

    recovery: Option<PathBuf>,
    /// 前回の起動で、処理中に異常終了したプラグイン（種類名）。
    pub(crate) plugin_crash: Option<String>,
    show: windows::Shown,
    screenshot: Option<Screenshot>,

    session_start: u64,
    active: Duration,
    last_activity: Instant,
    edits: u32,
    timelapse: bool,
    last_capture: Instant,
}

fn new_engine(ctx: &egui::Context, path: Option<&Path>) -> PreviewEngine {
    let ctx = ctx.clone();
    PreviewEngine::new(&base_dir(path), move || ctx.request_repaint())
}

pub(crate) fn base_dir(path: Option<&Path>) -> PathBuf {
    path.and_then(|p| p.parent()).filter(|p| !p.as_os_str().is_empty()).map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."))
}

impl App {
    fn new(ctx: &egui::Context, path: Option<PathBuf>, start_frame: i64, screenshot: Option<PathBuf>) -> Self {
        let mut app = App {
            project: demo_project(),
            shared: Arc::new(demo_project()),
            revision: 0,
            font_previews: Default::default(),
            path: None,
            dirty: false,
            unsaved_since_autosave: false,
            last_autosave: Instant::now(),
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: None,
            user: std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_else(|_| "自分".into()),
            comp_id: CompId::new("main"),
            frame: start_frame,
            selected: None,
            selected_footage: None,
            downsample: 2,
            playing: None,
            work_area: None,
            snap: true,
            hide_shy: false,
            only_animated: false,
            engine: new_engine(ctx, None),
            texture: None,
            overlay: None,
            shown: None,
            screening: false,
            zoom: 0.0,
            player: audio::Player::new(),
            audio_buf: Arc::default(),
            audio_sig: 0,
            audio_task: None,
            spectrogram: None,
            show_spectrogram: false,
            last_scrub_frame: -1,
            status: "デモプロジェクトを開いています".into(),
            expr_errors: (u64::MAX, Vec::new()),
            expr_open: None,
            expr_buf: String::new(),
            graph_path: None,
            export_ui: windows::ExportUi::default(),
            export_task: None,
            queue: Vec::new(),
            check: CheckResult::default(),
            check_task: None,
            diff: None,
            snapshot_name: "1次提出版".into(),
            review_text: String::new(),
            script_text: "// project を書き換えます。例:\nforEachLayer(function (l) { l.shy = false; });\n".into(),
            tool: tools::Tool::Select,
            viewer_drag: None,
            pen: None,
            active_mask: 0,
            active_vertex: None,
            tl_zoom: 1.0,
            tl_scroll: 0.0,
            tl_drag: None,
            tl_view: None,
            tl_left: timeline::LEFT_DEFAULT,
            tl_detail: false,
            tl_open: Default::default(),
            tl_search: String::new(),
            selected_keys: Vec::new(),
            marquee_hits: Vec::new(),
            key_clip: None,
            beat_ui: Default::default(),
            prefs: prefs::Prefs::load(),
            ease: Default::default(),
            graph_view: (0.0, 0.0),
            graph_view_for: None,
            keymap: keymap::Keymap::default(),
            keymap_capture: None,
            key_target: None,
            right_tab: 0,
            project_search: String::new(),
            channel: 0,
            show_guides: false,
            show_grid: false,
            hover_info: None,
            anchors: Default::default(),
            script: None,
            script_shot: None,
            // プラグインのエフェクトを、プロジェクトを開く前に使えるようにしておく
            plugin_report: {
                // ネイティブのプラグインは、利用者が許可していなければ読み込まない（WebAssembly はサンドボックスで動く）
                znd_render::plugins::set_native_allowed(prefs::Prefs::load().allow_native_plugins);
                znd_render::plugins::scan_default().errors
            },
            plugins: Default::default(),
            recovery: None,
            plugin_crash: None,
            show: windows::Shown::default(),
            screenshot: screenshot.map(|path| Screenshot { path, started: Instant::now(), requested: false }),
            session_start: unix_now(),
            active: Duration::ZERO,
            last_activity: Instant::now(),
            edits: 0,
            timelapse: false,
            last_capture: Instant::now(),
        };
        // 前回プラグインの処理中に落ちていたら尋ねる。記録を読んでから、今回の記録を始める
        app.plugin_crash = znd_render::plugins::guard::take_crash_record();
        if let Err(e) = znd_render::plugins::guard::install_crash_journal() {
            eprintln!("プラグインの異常終了の記録を始められません: {e}");
        }
        if let Some(p) = &app.player {
            p.set_offset_ms(app.prefs.av_offset_ms);
        }
        let (keymap, warnings) = keymap::Keymap::load();
        app.keymap = keymap;
        if !warnings.is_empty() {
            app.status = format!("ショートカットの設定: {}", warnings.join(" / "));
        }
        match path {
            Some(p) => {
                app.open(ctx, &p);
                app.frame = start_frame;
            }
            // 未保存のまま終了・異常終了した前回の作業
            None => app.recovery = io::newer_autosave(&app.autosave_base()),
        }
        if app.screenshot.is_some() {
            app.recovery = None;
        }
        app.publish();
        app
    }

    // ---------- プロジェクトの入れ替え・保存 ----------

    pub(crate) fn replace_project(&mut self, ctx: &egui::Context, project: Project, path: Option<PathBuf>) {
        self.comp_id = project.compositions.first().map(|c| c.id.clone()).unwrap_or_else(|| CompId::new(""));
        self.project = project;
        self.path = path;
        self.undo.clear();
        self.redo.clear();
        self.last_edit = None;
        self.selected = None;
        self.selected_footage = None;
        self.selected_keys.clear();
        self.key_target = None;
        self.tl_open.clear();
        self.frame = 0;
        self.stop();
        self.dirty = false;
        self.unsaved_since_autosave = false;
        self.check = CheckResult::default();
        self.diff = None;
        self.engine = new_engine(ctx, self.path.as_deref());
        self.engine.footage.attach_existing_proxies(&self.project);
        self.audio_sig = 0;
        self.publish();
    }

    pub(crate) fn open(&mut self, ctx: &egui::Context, path: &Path) {
        match io::load(path) {
            Ok(project) => {
                let issues = project.validate();
                let missing = znd_media::manage::missing_footage(&project, &base_dir(Some(path))).len();
                let fonts = znd_media::manage::missing_fonts(&project);
                self.replace_project(ctx, project, Some(path.to_path_buf()));
                self.recovery = io::newer_autosave(path);
                let mut notes = Vec::new();
                if !issues.is_empty() {
                    notes.push(format!("構造の問題 {} 件", issues.len()));
                }
                if missing > 0 {
                    notes.push(format!("欠落フッテージ {missing} 個（ファイル → 素材を再リンク）"));
                }
                if !fonts.is_empty() {
                    notes.push(format!("欠落フォント: {}", fonts.join("、")));
                }
                self.status = if notes.is_empty() {
                    format!("{} を開きました", path.display())
                } else {
                    format!("{} を開きました — {}", path.display(), notes.join(" / "))
                };
            }
            Err(e) => self.status = format!("開けません: {e}"),
        }
    }

    fn save_to(&mut self, path: PathBuf) {
        match io::save(&self.project, &path) {
            Ok(()) => {
                self.status = format!("{} に保存しました", path.display());
                self.path = Some(path);
                self.dirty = false;
                self.unsaved_since_autosave = false;
            }
            Err(e) => self.status = format!("保存に失敗しました: {e}"),
        }
    }

    pub(crate) fn save(&mut self) {
        match self.path.clone() {
            Some(p) => self.save_to(p),
            None => self.save_as(),
        }
    }

    pub(crate) fn save_as(&mut self) {
        if let Some(p) = rfd::FileDialog::new().add_filter("znd プロジェクト", &["json"]).set_file_name("untitled.znd.json").save_file()
        {
            self.save_to(p);
        }
    }

    /// 自動保存の基準パス。未保存プロジェクトは一時ディレクトリに退避する。
    pub(crate) fn autosave_base(&self) -> PathBuf {
        self.path.clone().unwrap_or_else(|| std::env::temp_dir().join("znd-untitled.znd.json"))
    }

    fn autosave_now(&mut self) {
        match io::autosave(&self.project, &self.autosave_base(), AUTOSAVE_GENERATIONS) {
            Ok(_) => self.unsaved_since_autosave = false,
            Err(e) => self.status = format!("自動保存に失敗しました: {e}"),
        }
        self.last_autosave = Instant::now();
    }

    // ---------- 編集と取り消し ----------

    /// 編集後の状態をレンダースレッドに公開する。
    fn publish(&mut self) {
        self.revision += 1;
        self.shared = Arc::new(self.project.clone());
    }

    /// プロジェクトは変えずに、プレビューを作り直させる（プラグインの有効・無効を切り替えたときなど）。
    pub(crate) fn publish_refresh(&mut self) {
        self.publish();
    }

    /// プロジェクトを変更する唯一の入口。`coalesce` が true なら、直前の同名編集と取り消し単位をまとめる。
    pub(crate) fn edit(&mut self, label: &str, coalesce: bool, f: impl FnOnce(&mut Project)) {
        let now = Instant::now();
        let merge = coalesce && self.last_edit.as_ref().is_some_and(|(l, t)| l == label && now.duration_since(*t) < COALESCE_WINDOW);
        if !merge {
            self.undo.push((label.to_string(), self.project.clone()));
            self.redo.clear();
            self.edits += 1;
        }
        f(&mut self.project);
        self.last_edit = Some((label.to_string(), now));
        self.dirty = true;
        self.unsaved_since_autosave = true;
        self.publish();
    }

    /// 現在のコンポを編集する。他の人がロックしているコンポは編集しない。
    pub(crate) fn edit_comp(&mut self, label: &str, coalesce: bool, f: impl FnOnce(&mut Composition)) {
        let id = self.comp_id.clone();
        if let Some(owner) = self.shared.comp(&id).and_then(|c| c.locked_by.clone()).filter(|o| o != &self.user) {
            self.status = format!("このコンポは {owner} がロックしているため編集できません");
            return;
        }
        self.edit(label, coalesce, |p| {
            if let Some(c) = p.comp_mut(&id) {
                f(c);
                // タイムラインの 1 行に並ぶレイヤーの番号を整える（並び替えで別の行と混ざらないように）
                edit::normalize_tracks(c);
            }
        });
    }

    pub(crate) fn edit_layer(&mut self, label: &str, coalesce: bool, layer: &LayerId, f: impl FnOnce(&mut Layer)) {
        let layer = layer.clone();
        self.edit_comp(label, coalesce, |c| {
            if let Some(l) = c.layer_mut(&layer) {
                f(l);
            }
        });
    }

    pub(crate) fn undo(&mut self) {
        if let Some((label, prev)) = self.undo.pop() {
            self.redo.push((label.clone(), std::mem::replace(&mut self.project, prev)));
            self.after_history_jump(&format!("取り消し: {label}"));
        }
    }

    pub(crate) fn redo(&mut self) {
        if let Some((label, next)) = self.redo.pop() {
            self.undo.push((label.clone(), std::mem::replace(&mut self.project, next)));
            self.after_history_jump(&format!("やり直し: {label}"));
        }
    }

    /// 履歴の `index` 番目の操作の直前まで戻る。
    pub(crate) fn jump_to_history(&mut self, index: usize) {
        while self.undo.len() > index {
            self.undo();
        }
    }

    fn after_history_jump(&mut self, status: &str) {
        self.last_edit = None;
        self.dirty = true;
        self.unsaved_since_autosave = true;
        if self.project.comp(&self.comp_id).is_none() {
            self.comp_id = self.project.compositions.first().map(|c| c.id.clone()).unwrap_or_else(|| CompId::new(""));
        }
        self.status = status.into();
        self.publish();
    }

    // ---------- レイヤー操作 ----------

    pub(crate) fn add_layer(&mut self, label: &str, name: &str, kind: LayerKind) {
        let shared = self.shared.clone();
        let Some(comp) = shared.comp(&self.comp_id) else { return };
        let (cw, ch) = (comp.width as f64, comp.height as f64);
        let (sw, sh) = shared.source_size(&kind).unwrap_or((0.0, 0.0));
        let id = shared.fresh_id("layer");
        let mut layer = Layer::new(id.clone(), name, kind, Transform::centered(sw, sh, cw, ch), comp.duration);
        // どのレイヤーも再生位置から始める（素材は素材の長さだけ、それ以外はコンポの終わりまで）
        let t = self.playhead_time(comp);
        place_at(&mut layer, t, comp.duration, &shared);
        if let LayerKind::Model { .. } = &layer.kind {
            layer.three_d = true;
        }
        self.edit_comp(label, false, |c| c.layers.insert(0, layer));
        self.selected = Some(LayerId::new(id));
    }

    /// 新しいレイヤーを置く時刻（再生ヘッド。コンポの最後のフレームより後ろにはしない）。
    pub(crate) fn playhead_time(&self, comp: &Composition) -> Time {
        let last = (comp.frame_count() - 1).max(0);
        comp.frame_rate.frame_to_time(self.frame.clamp(0, last))
    }

    /// ファイルを選んで読み込む。`sequence` が true なら、選んだ 1 枚を含む連番をまとめて 1 本の動画にする。
    pub(crate) fn import_footage_dialog(&mut self, sequence: bool) {
        let Some(path) = rfd::FileDialog::new().pick_file() else { return };
        self.import_path(&path, sequence);
    }

    /// 種類を見分けて読み込む（動画・静止画・音声・連番・PSD・SVG/PDF/AI・3D モデル）。
    pub(crate) fn import_path(&mut self, path: &Path, sequence: bool) {
        let comp_id = self.comp_id.clone();
        let opts = ImportOptions { sequence, at: self.frame_rate().frame_to_time(self.frame), ..Default::default() };
        let mut outcome = None;
        self.edit("素材の読み込み", false, |p| outcome = Some(import_file(p, &comp_id, path, opts)));
        match outcome {
            Some(Ok(out)) => {
                self.selected = out.layers.last().cloned();
                self.selected_footage = None;
                let rights = if out.footage.is_empty() { "" } else { "（権利情報: 未確認）" };
                self.status = format!("{}{rights}{}", out.summary, out.warnings.iter().map(|w| format!(" — {w}")).collect::<String>());
            }
            Some(Err(e)) => {
                // 何も変わっていないので取り消し履歴に残さない
                self.undo.pop();
                self.publish();
                self.status = format!("読み込めません: {e}");
            }
            None => {}
        }
    }

    // ---------- 音声 ----------

    /// 音に関わる設定だけから作る署名。これが変わったときだけミックスし直す。
    fn audio_signature(&self) -> u64 {
        let Some(comp) = self.shared.comp(&self.comp_id) else { return 0 };
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        let mut feed = |s: &str| s.bytes().for_each(|b| h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3));
        feed(&comp.id.0);
        fn walk(p: &Project, c: &Composition, feed: &mut dyn FnMut(&str), depth: usize) {
            for l in &c.layers {
                let nested = match &l.kind {
                    LayerKind::Precomp { comp } if depth < 16 => p.comp(comp),
                    _ => None,
                };
                // 音声エフェクト（プラグイン）のパラメータが変わったときもミックスし直す
                let audio_fx: Vec<&EffectInstance> =
                    l.effects.iter().filter(|e| znd_core::effect::audio_processor(&e.kind).is_some()).collect();
                if znd_audio::layer_footage(p, l).is_some() || nested.is_some() || !audio_fx.is_empty() {
                    feed(
                        &serde_json::to_string(&(&l.kind, l.in_point, l.out_point, l.start_time, l.speed, &l.audio, &l.time_remap, l.solo))
                            .unwrap_or_default(),
                    );
                    feed(&serde_json::to_string(&audio_fx).unwrap_or_default());
                }
                if let Some(n) = nested {
                    walk(p, n, feed, depth + 1);
                }
            }
        }
        walk(&self.shared, comp, &mut feed, 0);
        h
    }

    fn tick_audio(&mut self, ctx: &egui::Context) {
        if let Some(task) = &self.audio_task {
            if let Some(result) = task.take() {
                self.audio_task = None;
                match result {
                    Ok((sig, buf)) if sig == self.audio_signature() => {
                        self.audio_buf = Arc::new(buf);
                        if let Some(p) = &self.player {
                            p.set_buffer(self.audio_buf.clone());
                        }
                        self.spectrogram = None;
                    }
                    Ok(_) => {}
                    Err(e) => self.status = format!("音声を読み込めません: {e}"),
                }
            }
        }
        let sig = self.audio_signature();
        if sig != self.audio_sig && self.audio_task.is_none() {
            self.audio_sig = sig;
            let (project, comp, clips) = (self.shared.clone(), self.comp_id.clone(), self.engine.clips.clone());
            self.audio_task = Some(Task::spawn("音声をミックス中", ctx, move |_| {
                let mixer = Mixer::new(&project, &*clips);
                let frames = project.comp(&comp).map(|c| mixer.comp_frames(c)).unwrap_or(0);
                Ok((sig, mixer.mix(&comp, 0, frames)?))
            }));
        }
    }

    pub(crate) fn has_audio(&self) -> bool {
        self.player.as_ref().is_some_and(|p| p.has_audio())
    }

    // ---------- 再生 ----------

    pub(crate) fn total_frames(&self) -> i64 {
        self.shared.comp(&self.comp_id).map(|c| c.frame_count()).unwrap_or(1).max(1)
    }

    pub(crate) fn frame_rate(&self) -> FrameRate {
        self.shared.comp(&self.comp_id).map(|c| c.frame_rate).unwrap_or(FrameRate::FPS_30)
    }

    pub(crate) fn stop(&mut self) {
        self.playing = None;
        if let Some(p) = &self.player {
            p.stop();
        }
    }

    /// 再生ヘッドを動かす。`snap` が true でスナップが有効なら、最寄りのビート・マーカーに吸着する。
    pub(crate) fn set_frame(&mut self, frame: i64, snap: bool) {
        let mut frame = frame.clamp(0, self.total_frames() - 1);
        if snap && self.snap {
            if let Some(comp) = self.shared.comp(&self.comp_id) {
                let fr = comp.frame_rate;
                let t = fr.frame_to_time(frame);
                let mut best: Option<(i64, i64)> = None;
                let mut consider = |cand: Time| {
                    let f = fr.time_to_frame(beat::nearest_frame(cand, fr));
                    let d = (f - frame).abs();
                    // 3 フレーム以内なら吸着する
                    if d <= 3 && best.is_none_or(|b| d < b.0) {
                        best = Some((d, f));
                    }
                };
                if let Some(g) = &comp.beat_grid {
                    consider(g.snap(t));
                }
                comp.markers.iter().for_each(|m| consider(m.time));
                if let Some((_, f)) = best {
                    frame = f.clamp(0, self.total_frames() - 1);
                }
            }
        }
        self.frame = frame;
        self.stop();
    }

    pub(crate) fn toggle_play(&mut self) {
        if self.playing.is_some() {
            self.stop();
            return;
        }
        // ワークエリアの外にいたら先頭から
        if let Some((a, b)) = self.work_area {
            if self.frame < a || self.frame >= b {
                self.frame = a;
            }
        }
        self.playing = Some((Instant::now(), self.frame));
        let fr = self.frame_rate();
        if let Some(p) = &self.player {
            let (a, b) = self.work_area.unwrap_or((0, self.total_frames()));
            p.play_from(
                time_to_sample(fr.frame_to_time(self.frame)),
                Some((time_to_sample(fr.frame_to_time(a)), time_to_sample(fr.frame_to_time(b)))),
            );
        }
    }

    fn tick_playback(&mut self, ctx: &egui::Context) {
        let fr = self.frame_rate();
        if let Some((started, origin)) = self.playing {
            // 音があるときは、音声デバイスの再生位置を時計にする（映像が音に合わせて進む）
            let audio_clock = self.player.as_ref().filter(|p| p.is_playing() && p.has_audio()).map(|p| p.position());
            let frame = match audio_clock {
                Some(sample) => fr.time_to_frame(znd_audio::sample_to_time(sample)),
                None => {
                    let advanced = (started.elapsed().as_secs_f64() * fr.fps()).floor() as i64;
                    let (a, b) = self.work_area.unwrap_or((0, self.total_frames()));
                    a + (origin - a + advanced).rem_euclid((b - a).max(1))
                }
            };
            self.frame = frame.clamp(0, self.total_frames() - 1);
            ctx.request_repaint();
        } else if self.frame != self.last_scrub_frame {
            // スクラブ: フレームが変わったら、その位置の音を少しだけ鳴らす
            if let Some(p) = self.player.as_ref().filter(|p| self.last_scrub_frame >= 0 && p.has_audio()) {
                let len = (SAMPLE_RATE as f64 / fr.fps()).ceil() as i64 * 2;
                p.scrub(time_to_sample(fr.frame_to_time(self.frame)), len);
            }
        }
        self.last_scrub_frame = self.frame;
    }

    pub(crate) fn add_marker(&mut self, comment: &str) {
        let t = self.frame_rate().frame_to_time(self.frame);
        let comment = comment.to_string();
        // 再生中に連打できる（タップ入力）よう、再生は止めない
        self.edit_comp("マーカー追加", false, |c| {
            if !c.markers.iter().any(|m| m.time == t) {
                c.markers.push(Marker { time: t, duration: Time::ZERO, comment });
                c.markers.sort_by_key(|m| m.time);
            }
        });
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        // 割り当ての変更中は、次に押したキーを割り当てに使う
        if let Some(action) = self.keymap_capture {
            let hit = ctx.input_mut(|i| {
                let mut hit = None;
                i.events.retain(|e| match e {
                    egui::Event::Key { key, pressed: true, modifiers, .. } if hit.is_none() => {
                        hit = Some((*key, *modifiers));
                        false
                    }
                    _ => true,
                });
                hit
            });
            if let Some((key, m)) = hit {
                self.keymap_capture = None;
                if key != egui::Key::Escape || m.any() {
                    let chord = keymap::Chord::from_event(key, m);
                    let mut chords = self.keymap.chords(action);
                    if !chords.contains(&chord) {
                        chords.push(chord);
                    }
                    self.keymap.set(action, chords);
                    self.save_keymap();
                }
            }
            return;
        }
        let typing = ctx.wants_keyboard_input();
        let actions = ctx.input_mut(|i| keymap::take_actions(&self.keymap, &mut i.events, typing));
        for a in actions {
            self.run_action(a);
        }
        // 文字入力中は再生・コマ送りキーを奪わない
        if typing {
            return;
        }
        // キーフレームのコピー・貼り付け
        self.key_clipboard_events(ctx);
    }

    /// メニューに添える、操作のショートカットの表記（割り当てが無ければ空）。
    pub(crate) fn shortcut_text(&self, action: keymap::Action) -> String {
        self.keymap.chords(action).first().map(|c| c.display()).unwrap_or_default()
    }

    pub(crate) fn save_keymap(&mut self) {
        match self.keymap.save() {
            Ok(p) => self.status = format!("ショートカットを保存しました（{}）", p.display()),
            Err(e) => self.status = format!("ショートカットを保存できません: {e}"),
        }
    }

    /// ショートカット・メニューから呼ばれる操作。
    pub(crate) fn run_action(&mut self, action: keymap::Action) {
        use keymap::Action::*;
        let fr = self.frame_rate();
        let now = fr.frame_to_time(self.frame);
        let layer = self.selected.as_ref().and_then(|id| self.shared.comp(&self.comp_id)?.layer(id).cloned());
        match action {
            Undo => self.undo(),
            Redo => self.redo(),
            Save => self.save(),
            Play => self.toggle_play(),
            NextFrame => self.set_frame(self.frame + 1, false),
            PrevFrame => self.set_frame(self.frame - 1, false),
            NextFrame10 => self.set_frame(self.frame + 10, false),
            PrevFrame10 => self.set_frame(self.frame - 10, false),
            FirstFrame => self.set_frame(0, false),
            LastFrame => self.set_frame(i64::MAX, false),
            PrevKey | NextKey => {
                let mut times: Vec<i64> = Vec::new();
                if let Some(l) = &layer {
                    visit::visit_layer(l, &mut |_, p| times.extend(p.key_times().iter().map(|t| fr.time_to_frame(*t))));
                }
                let target = if action == PrevKey {
                    times.iter().filter(|f| **f < self.frame).max()
                } else {
                    times.iter().filter(|f| **f > self.frame).min()
                };
                if let Some(f) = target {
                    self.set_frame(*f, false);
                }
            }
            GoInPoint => {
                if let Some(l) = &layer {
                    self.set_frame(fr.time_to_frame(l.in_point), false);
                }
            }
            GoOutPoint => {
                if let Some(l) = &layer {
                    self.set_frame(fr.frame_count(l.out_point) - 1, false);
                }
            }
            Marker => self.add_marker(""),
            WorkStart => {
                let end = self.work_area.map(|w| w.1).unwrap_or(self.total_frames()).max(self.frame + 1);
                self.work_area = Some((self.frame, end));
            }
            WorkEnd => {
                let start = self.work_area.map(|w| w.0).unwrap_or(0).min(self.frame);
                self.work_area = Some((start, self.frame + 1));
            }
            ToolSelect => self.set_tool(tools::Tool::Select),
            ToolHand => self.set_tool(tools::Tool::Hand),
            ToolZoom => self.set_tool(tools::Tool::Zoom),
            ToolRotate => self.set_tool(tools::Tool::Rotate),
            ToolPen => self.set_tool(tools::Tool::Pen),
            ToolRect => self.set_tool(tools::Tool::RectMask),
            ToolEllipse => self.set_tool(tools::Tool::EllipseMask),
            Delete => {
                // 選択中のキーフレームを削除（キーフレームを選んでいなければ、選択中のレイヤーを削除）
                if self.active_vertex.is_none() && self.pen.is_none() {
                    if !self.selected_keys.is_empty() {
                        self.delete_selected_keys();
                    } else if let Some(id) = self.selected.clone().filter(|_| self.selected_footage.is_none()) {
                        self.delete_layer(&id);
                    }
                }
            }
            Duplicate => {
                if let Some(id) = self.selected.clone() {
                    self.duplicate_layer(&id);
                }
            }
            SplitLayer => {
                if let Some(l) = layer.filter(|l| !l.locked) {
                    let new_id = LayerId::new(self.shared.fresh_id("layer"));
                    let (id, created) = (l.id.clone(), new_id.clone());
                    let mut done = false;
                    self.edit_comp("レイヤーを分割", false, |c| done = edit::split_layer(c, &id, now, new_id));
                    if done {
                        self.selected = Some(created);
                        self.selected_keys.clear();
                        self.status = format!("「{}」を再生ヘッドで分割しました", l.name);
                    } else {
                        self.status = "再生ヘッドがレイヤーのイン点とアウト点の間にありません".into();
                    }
                }
            }
            TrimInToPlayhead | TrimOutToPlayhead | MoveInToPlayhead | MoveOutToPlayhead => {
                if let Some(l) = layer.filter(|l| !l.locked) {
                    let label = info_label(action);
                    self.edit_layer(label, false, &l.id, |l| match action {
                        TrimInToPlayhead => edit::trim_in(l, now, fr),
                        TrimOutToPlayhead => edit::trim_out(l, now + fr.frame_to_time(1), fr),
                        MoveInToPlayhead => edit::shift_layer(l, now - l.in_point),
                        _ => edit::shift_layer(l, now + fr.frame_to_time(1) - l.out_point),
                    });
                }
            }
            Hold1 | Hold2 | Hold3 | Hold4 => {
                let n = match action {
                    Hold1 => 1,
                    Hold2 => 2,
                    Hold3 => 3,
                    _ => 4,
                };
                if let Some(id) = self.selected.clone() {
                    self.edit_layer("コマ打ち", false, &id, |l| l.hold_frames = n);
                    self.status = if n == 1 { "コマ打ちを解除しました".into() } else { format!("{n} コマ打ちにしました") };
                }
            }
            SelectAllKeys => self.select_all_keys(),
            EasyEase => self.style_selected_keys("イージーイーズ", panels::KeyKind::EasyEase.style()),
            EaseIn => self.style_selected_keys("イーズイン", panels::KeyKind::EaseIn.style()),
            EaseOut => self.style_selected_keys("イーズアウト", panels::KeyKind::EaseOut.style()),
            LinearKeys => self.style_selected_keys("リニア", panels::KeyKind::Linear.style()),
            HoldKeys => self.style_selected_keys("停止", panels::KeyKind::Hold.style()),
            ReverseKeys => self.reverse_selected_keys(),
            ToggleSnap => {
                self.snap = !self.snap;
                self.status = if self.snap { "スナップ: オン".into() } else { "スナップ: オフ".into() };
            }
            TimelineZoomIn => self.tl_zoom *= 1.25,
            TimelineZoomOut => self.tl_zoom /= 1.25,
            ShowShortcuts => self.show.shortcuts = !self.show.shortcuts,
            ApplyEase => {
                let c = self.ease.current;
                self.apply_ease(Some(c), false);
            }
        }
    }

    // ---------- 表示 ----------

    fn viewer(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let shared = self.shared.clone();
        let Some(comp) = shared.comp(&self.comp_id) else {
            ui.centered_and_justified(|ui| ui.label("コンポジションがありません"));
            return;
        };
        // 現在フレームがキャッシュにあればテクスチャを差し替える。無ければ直前の絵を出し続ける。
        let key = (self.revision, self.frame, self.downsample);
        if self.shown != Some(key) {
            if let Some(p) = self.engine.get(self.revision, self.frame, self.downsample) {
                let image = channel_image(&p, self.channel);
                match &mut self.texture {
                    Some(t) => t.set(image, egui::TextureOptions::LINEAR),
                    None => self.texture = Some(ctx.load_texture("viewer", image, egui::TextureOptions::LINEAR)),
                }
                self.overlay = (p.screening.overlay.len() == p.width * p.height * 4).then(|| {
                    let img = egui::ColorImage::from_rgba_unmultiplied([p.width, p.height], &p.screening.overlay);
                    ctx.load_texture("screening", img, egui::TextureOptions::NEAREST)
                });
                if self.screening {
                    let s = &p.screening;
                    let pct = |v: f64| format!("{:.1}%", v * 100.0);
                    self.status = format!(
                        "上映プレビュー: 白飛び {} / 黒つぶれ {} / バンディングの恐れ {}",
                        pct(s.clipped_white),
                        pct(s.crushed_black),
                        pct(s.banding_risk)
                    );
                }
                self.shown = Some(key);
            }
        }

        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            let avail = ui.available_size();
            let scale = if self.zoom > 0.0 { self.zoom } else { (avail.x / comp.width as f32).min(avail.y / comp.height as f32) };
            let size = egui::vec2(comp.width as f32 * scale, comp.height as f32 * scale);
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(size.x.max(avail.x), size.y.max(avail.y)), egui::Sense::click_and_drag());
            // 上映プレビューでは周囲を真っ黒にする（暗い会場での見え方に近づける）
            ui.painter().rect_filled(rect, 0.0, if self.screening { egui::Color32::BLACK } else { theme::INPUT });
            let img = egui::Rect::from_center_size(rect.center(), size);
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            match (&self.diff, &self.texture) {
                (Some((tex, _)), _) => {
                    ui.painter().image(tex.id(), img, uv, egui::Color32::WHITE);
                }
                (None, Some(tex)) => {
                    ui.painter().image(tex.id(), img, uv, egui::Color32::WHITE);
                    if let Some(o) = self.overlay.as_ref().filter(|_| self.screening) {
                        ui.painter().image(o.id(), img, uv, egui::Color32::WHITE);
                    }
                }
                _ => {}
            }
            // グリッドとセーフマージン（アクションセーフ 90%・タイトルセーフ 80%）
            let overlay = ui.painter_at(img.intersect(rect));
            if self.show_grid {
                let step = (comp.width.max(comp.height) as f32 / 16.0).max(8.0) * scale;
                let line = egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(26));
                let mut x = img.left() + step;
                while x < img.right() {
                    overlay.vline(x, img.y_range(), line);
                    x += step;
                }
                let mut y = img.top() + step;
                while y < img.bottom() {
                    overlay.hline(img.x_range(), y, line);
                    y += step;
                }
            }
            if self.show_guides {
                let line = egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(90));
                for k in [0.9, 0.8] {
                    overlay.rect_stroke(egui::Rect::from_center_size(img.center(), img.size() * k), 0.0, line);
                }
                let c = img.center();
                overlay.hline((c.x - 10.0)..=(c.x + 10.0), c.y, line);
                overlay.vline(c.x, (c.y - 10.0)..=(c.y + 10.0), line);
            }
            // ポインタの下の座標と色（情報パネル用）
            self.hover_info = resp.hover_pos().filter(|h| img.contains(*h)).and_then(|h| {
                let pos = [((h.x - img.left()) / scale) as f64, ((h.y - img.top()) / scale) as f64];
                let p = self.engine.get(self.revision, self.frame, self.downsample)?;
                let (px, py) = (
                    ((pos[0] / comp.width as f64 * p.width as f64) as usize).min(p.width.saturating_sub(1)),
                    ((pos[1] / comp.height as f64 * p.height as f64) as usize).min(p.height.saturating_sub(1)),
                );
                let rgba: [u8; 4] = p.rgba.get((py * p.width + px) * 4..(py * p.width + px) * 4 + 4)?.try_into().ok()?;
                Some((pos, rgba))
            });
            // この時刻のレビューコメントの位置
            let t = comp.frame_rate.frame_to_time(self.frame);
            for r in shared.reviews.iter().filter(|r| r.comp == comp.id && !r.resolved && r.time == t) {
                if let Some(p) = r.point {
                    let pos = img.min + egui::vec2(p[0] as f32 * scale, p[1] as f32 * scale);
                    ui.painter().circle_stroke(pos, 10.0, egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(80, 180, 255)));
                    let text = format!("{}: {}", r.author, r.text);
                    ui.painter().text(
                        pos + egui::vec2(14.0, -8.0),
                        egui::Align2::LEFT_TOP,
                        text,
                        egui::FontId::proportional(11.5),
                        egui::Color32::from_rgb(160, 220, 255),
                    );
                }
            }
            // 選択・移動、モーションパス、マスクの直接操作（差分表示中は無効）
            self.anchors.insert("viewer".into(), img);
            if self.diff.is_none() {
                self.viewer_tools(ui, &resp, img, scale);
            }
            if let Some(e) = self.engine.error() {
                ui.painter().text(
                    rect.left_top() + egui::vec2(8.0, 8.0),
                    egui::Align2::LEFT_TOP,
                    e,
                    egui::FontId::proportional(11.5),
                    theme::ERROR,
                );
            }
            if let Some((_, label)) = &self.diff {
                ui.painter().text(
                    rect.left_top() + egui::vec2(8.0, 8.0),
                    egui::Align2::LEFT_TOP,
                    label,
                    egui::FontId::proportional(11.5),
                    theme::WARN,
                );
            }
        });
    }

    fn tick_screenshot(&mut self, ctx: &egui::Context) {
        let frame_shown = self.shown.is_some_and(|s| s.1 == self.frame);
        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let [w, h] = image.size;
            // 操作スクリプトの撮影、起動時のスクリーンショット指定、タイムラプス録画のいずれか
            let (path, close) = match (self.script_shot.take(), &self.screenshot) {
                (Some(p), _) => (p, false),
                (None, Some(s)) => (s.path.clone(), true),
                (None, None) => {
                    let dir = base_dir(self.path.as_deref()).join(".znd-timelapse");
                    let _ = std::fs::create_dir_all(&dir);
                    (dir.join(format!("{}.png", unix_now())), false)
                }
            };
            if let Err(e) = image::save_buffer(&path, image.as_raw(), w as u32, h as u32, image::ExtendedColorType::Rgba8) {
                eprintln!("スクリーンショットを保存できません: {e}");
            }
            if close {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            return;
        }
        if self.script.is_some() {
            return;
        }
        if let Some(shot) = &mut self.screenshot {
            if !shot.requested && (frame_shown || shot.started.elapsed() > Duration::from_secs(20)) {
                shot.requested = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
            }
            ctx.request_repaint();
        } else if self.timelapse && self.last_capture.elapsed() > Duration::from_secs(30) && self.last_activity.elapsed() < IDLE_LIMIT {
            // タイムラプス: 作業している間だけ、30 秒おきに画面を保存する
            self.last_capture = Instant::now();
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
        }
    }

    /// 制作ログ用に、操作が続いている時間を数える。
    fn tick_session(&mut self, ctx: &egui::Context) {
        let dt = ctx.input(|i| i.unstable_dt).min(1.0);
        let busy = ctx.input(|i| i.pointer.is_moving() || i.pointer.any_down() || !i.events.is_empty());
        if busy {
            self.last_activity = Instant::now();
        }
        if self.last_activity.elapsed() < IDLE_LIMIT {
            self.active += Duration::from_secs_f32(dt);
        }
    }
}

/// 表示用の画像。チャンネルを選んでいるときは、その成分をグレースケールで出す。
fn channel_image(p: &preview::Preview, channel: u8) -> egui::ColorImage {
    if channel == 0 {
        return egui::ColorImage::from_rgba_unmultiplied([p.width, p.height], &p.rgba);
    }
    let pixels = p
        .rgba
        .chunks_exact(4)
        .map(|c| {
            let v = match channel {
                1..=3 => c[channel as usize - 1],
                _ => (0.2126 * c[0] as f32 + 0.7152 * c[1] as f32 + 0.0722 * c[2] as f32) as u8,
            };
            egui::Color32::from_gray(v)
        })
        .collect();
    egui::ColorImage { size: [p.width, p.height], pixels }
}

pub(crate) fn resolution_label(ds: u32) -> &'static str {
    match ds {
        1 => "フル",
        2 => "1/2",
        _ => "1/4",
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.shortcuts(ctx);
        self.tick_session(ctx);
        self.tick_audio(ctx);
        self.tick_playback(ctx);
        self.poll_tasks(ctx);
        if self.unsaved_since_autosave && self.last_autosave.elapsed() >= AUTOSAVE_INTERVAL {
            self.autosave_now();
        }
        if self.unsaved_since_autosave {
            // 操作が止まっていても自動保存の時刻が来たら起きる
            ctx.request_repaint_after(AUTOSAVE_INTERVAL);
        }
        self.engine.request(&self.shared, self.revision, &self.comp_id, self.frame, self.downsample);
        // 異常終了したときの退避先（保存先が変わることもあるので毎フレーム覚え直す。Arc の複製なので軽い）
        rescue::remember(&self.shared, &self.autosave_base());

        // エクスプレッションのエラーは、編集があったときだけ検査し直す
        if self.expr_errors.0 != self.revision {
            let t = self.frame_rate().frame_to_time(self.frame);
            self.expr_errors = (self.revision, znd_script::ExprEngine::new().check(&self.shared, &self.comp_id, t));
        }
        let revision = self.revision;
        self.workspace(ctx);
        self.windows(ctx);
        self.tick_script(ctx);
        self.tick_screenshot(ctx);
        self.tick_plugins(ctx);
        // このフレームの UI 操作で編集が入った（数値を入力して Enter など）。次の入力を待たずに描き直しを頼む。
        // そうしないと、ポインタを動かすかタイムラインを動かすまでプレビューが古いまま残る。
        if self.revision != revision {
            self.engine.request(&self.shared, self.revision, &self.comp_id, self.frame, self.downsample);
            ctx.request_repaint();
        }
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        if let Some(script) = &mut self.script {
            script.inject(raw);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // 保存せずに閉じても、次回起動時に復旧できるようにする
        if self.dirty {
            self.autosave_now();
        }
        // 制作ログに今回の作業を記録する（保存済みのプロジェクトのみ）
        if let Some(path) = &self.path {
            if self.edits > 0 {
                let entry = log::SessionEntry {
                    started: self.session_start,
                    seconds: self.active.as_secs(),
                    edits: self.edits,
                    user: self.user.clone(),
                };
                let _ = log::append_session(path, &entry);
            }
        }
    }
}

/// レイヤーを時刻 `t` から始まるように置く。素材は素材の長さだけ、それ以外はコンポの終わりまで。
pub(crate) fn place_at(layer: &mut Layer, t: Time, comp_duration: Time, project: &Project) {
    let len = match &layer.kind {
        LayerKind::Footage { footage } | LayerKind::Audio { footage } => {
            project.footage_item(footage).and_then(|f| f.duration.or(f.audio.map(|a| a.duration)))
        }
        _ => None,
    };
    layer.in_point = t;
    layer.start_time = t;
    layer.out_point = len.map(|d| (t + d).min(comp_duration)).unwrap_or(comp_duration);
}
