//! バックグラウンドで動く重い処理: 書き出し（キューを含む）と提出前チェック。
//!
//! 処理は別スレッドで走り、画面は `task_status` を数百ミリ秒おきに尋ねて進捗と結果を受け取る。
//! 同時に動かすのは 1 つだけ（書き出し中も編集は続けられる）。

use crate::{base_dir, lock, Shared, Studio};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::State;
use znd_core::*;
use znd_media::{ExportContext, ExportSettings, Format};
use znd_render::plugins::hooks;

/// 組み込みの書き出し形式（画面の選択欄の順）。この後ろにプラグインの形式が並ぶ。
const FORMATS: [&str; 10] = [
    "H.264（mp4）",
    "上映提出用 H.264（高画質・レポート付き）",
    "H.265（mp4、10bit）",
    "H.265 2 パス（目標ビットレート）",
    "ProRes 422（mov）",
    "ProRes 4444（mov、アルファ付き）",
    "PNG 連番",
    "PNG 連番（16bit）",
    "EXR 連番（リニア float）",
    "GIF（1/2 解像度）",
];

/// 画面の書き出しの設定。
#[derive(Deserialize, Serialize, Clone)]
pub struct ExportUi {
    /// `FORMATS` の番号。超えた分はプラグインの書き出し形式。
    format: usize,
    crf: u8,
    kbps: u32,
    audio: bool,
    /// 前後の黒味（秒）。
    pad: f64,
    transparent: bool,
    /// ワークエリアだけ書き出すときの範囲。
    range: Option<(i64, i64)>,
    /// 静止画の形式で書き出すフレーム。
    frame: i64,
    /// プラグインの書き出し形式のパラメータ（パラメータ ID → 値）。
    #[serde(default)]
    values: HashMap<String, String>,
}

impl ExportUi {
    fn plugin_exporter(&self) -> Option<hooks::Exporter> {
        self.format.checked_sub(FORMATS.len()).and_then(|i| hooks::exporters().get(i).cloned())
    }

    fn settings(&self) -> ExportSettings {
        if let Some(ex) = self.plugin_exporter() {
            let mut s = ExportSettings::new(Format::from_exporter(&ex, &self.values));
            s.audio = self.audio || ex.audio_only;
            s.transparent = self.transparent;
            if ex.single_frame {
                s.range = Some((self.frame, self.frame + 1));
            } else {
                s.pad_head = self.pad;
                s.pad_tail = self.pad;
                s.range = self.range;
            }
            return s;
        }
        let mut s = match self.format {
            0 => ExportSettings::new(Format::H264 { crf: self.crf, preset: "slow".into() }),
            1 => ExportSettings::screening(self.pad),
            2 => ExportSettings::new(Format::H265 { crf: self.crf.max(10), preset: "medium".into(), two_pass_kbps: None }),
            3 => ExportSettings::new(Format::H265 { crf: 18, preset: "medium".into(), two_pass_kbps: Some(self.kbps) }),
            4 => ExportSettings::new(Format::ProRes422),
            5 => ExportSettings::new(Format::ProRes4444),
            6 => ExportSettings::new(Format::PngSequence { sixteen_bit: false }),
            7 => ExportSettings::new(Format::PngSequence { sixteen_bit: true }),
            8 => ExportSettings::new(Format::ExrSequence),
            _ => ExportSettings { downsample: 2, ..ExportSettings::new(Format::Gif) },
        };
        s.audio = self.audio;
        s.pad_head = self.pad;
        s.pad_tail = self.pad;
        s.transparent = self.transparent;
        s.range = self.range;
        s
    }
}

#[derive(Serialize)]
pub struct ParamInfo {
    id: String,
    label: String,
    /// "number" / "choice" / "text" / "bool" / "args"
    kind: String,
    default: String,
    min: Option<f64>,
    max: Option<f64>,
    /// 選択肢の（表示名, 値）。
    choices: Vec<(String, String)>,
}

#[derive(Serialize)]
pub struct FormatInfo {
    label: String,
    description: String,
    plugin: bool,
    /// 透過で書き出せる。
    alpha: bool,
    /// CRF（画質）を選べる。
    crf: bool,
    /// 目標ビットレートを選べる。
    kbps: bool,
    single_frame: bool,
    params: Vec<ParamInfo>,
}

#[tauri::command]
pub fn export_formats() -> Vec<FormatInfo> {
    let mut out: Vec<FormatInfo> = FORMATS
        .iter()
        .enumerate()
        .map(|(i, f)| FormatInfo {
            label: f.to_string(),
            description: String::new(),
            plugin: false,
            alpha: (5..=8).contains(&i),
            crf: matches!(i, 0 | 2),
            kbps: i == 3,
            single_frame: false,
            params: Vec::new(),
        })
        .collect();
    for ex in hooks::exporters() {
        out.push(FormatInfo {
            label: ex.label.clone(),
            description: ex.description.clone(),
            plugin: true,
            alpha: ex.alpha,
            crf: false,
            kbps: false,
            single_frame: ex.single_frame,
            params: ex
                .params
                .iter()
                .map(|p| ParamInfo {
                    id: p.id.clone(),
                    label: p.label.clone(),
                    kind: p.kind.clone(),
                    default: p.default_value(),
                    min: p.min,
                    max: p.max,
                    choices: p.choices(),
                })
                .collect(),
        });
    }
    out
}

/// 権利が確認できていない素材（書き出す前に確かめる）。
#[tauri::command]
pub fn export_warnings(state: State<'_, Shared>, comp: String) -> Vec<String> {
    rights::export_warnings(&lock(&state).project, &CompId::new(comp))
}

/// 書き出し先を選ぶ。拡張子は形式から決める。
#[tauri::command]
pub fn export_pick(state: State<'_, Shared>, comp: String, settings: ExportUi) -> Option<String> {
    let name = lock(&state).project.comp(&CompId::new(comp)).map(|c| c.name.clone()).unwrap_or_else(|| "export".into());
    let ext = settings.settings().format.extension().to_string();
    rfd::FileDialog::new().set_file_name(format!("{name}.{ext}")).save_file().map(|p| p.display().to_string())
}

/// 書き出し 1 件。
#[derive(Deserialize, Serialize, Clone)]
pub struct Job {
    comp: String,
    path: String,
    settings: ExportUi,
}

// ---------- 実行中の処理 ----------

pub struct Running {
    label: String,
    done: Arc<AtomicI64>,
    total: Arc<AtomicI64>,
    cancel: Arc<AtomicBool>,
    result: Arc<Mutex<Option<Finished>>>,
}

#[derive(Serialize, Clone)]
pub struct Finished {
    /// "export" / "check"
    kind: &'static str,
    ok: bool,
    message: String,
    check: Option<Check>,
}

/// 提出前チェックの結果。
#[derive(Serialize, Clone, Default)]
pub struct Check {
    /// (問題なしか, 文面)。
    lines: Vec<(bool, String)>,
    /// 明滅の区間 [開始, 終了] フレームと説明（タイムラインに赤で出す）。
    flashes: Vec<(i64, i64, String)>,
}

fn spawn(s: &mut Studio, label: String, kind: &'static str, f: impl FnOnce(&mut dyn FnMut(i64, i64) -> bool) -> Result<(String, Option<Check>), String> + Send + 'static) -> Result<(), String> {
    if s.task.is_some() {
        return Err("ほかの処理が終わるまでお待ちください".into());
    }
    let running = Running { label, done: Arc::default(), total: Arc::default(), cancel: Arc::default(), result: Arc::default() };
    let (done, total, cancel, result) = (running.done.clone(), running.total.clone(), running.cancel.clone(), running.result.clone());
    std::thread::Builder::new()
        .name(format!("znd-{kind}"))
        .spawn(move || {
            let mut report = |d: i64, t: i64| {
                done.store(d, Ordering::Relaxed);
                total.store(t, Ordering::Relaxed);
                !cancel.load(Ordering::Relaxed)
            };
            // 内部エラーでパニックしても、作業中のアプリは止めず、失敗として伝える
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut report)))
                .unwrap_or_else(|e| Err(format!("内部エラーで中断しました: {}", znd_preview::panic_text(&*e))));
            let finished = match r {
                Ok((message, check)) => Finished { kind, ok: true, message, check },
                Err(message) => Finished { kind, ok: false, message, check: None },
            };
            *result.lock().unwrap_or_else(|e| e.into_inner()) = Some(finished);
        })
        .map_err(|e| e.to_string())?;
    s.task = Some(running);
    Ok(())
}

#[derive(Serialize)]
pub struct TaskStatus {
    /// 実行中なら（名前, 進み具合 0..1）。
    running: Option<(String, f32)>,
    /// 終わったもの（一度だけ返す）。
    finished: Option<Finished>,
}

#[tauri::command]
pub fn task_status(state: State<'_, Shared>) -> TaskStatus {
    let mut s = lock(&state);
    let finished = s.task.as_ref().and_then(|t| t.result.lock().unwrap_or_else(|e| e.into_inner()).take());
    if let Some(f) = &finished {
        s.task = None;
        if f.kind == "check" {
            s.check = f.check.clone();
        }
        if f.kind == "export" && f.ok {
            znd_media::notify("znd", "書き出しが終わりました");
        }
    }
    let running = s.task.as_ref().map(|t| (t.label.clone(), t.done.load(Ordering::Relaxed) as f32 / t.total.load(Ordering::Relaxed).max(1) as f32));
    TaskStatus { running, finished }
}

#[tauri::command]
pub fn task_cancel(state: State<'_, Shared>) {
    if let Some(t) = &lock(&state).task {
        t.cancel.store(true, Ordering::Relaxed);
    }
}

/// 書き出す（1 件、またはキューのすべて）。`report` なら、それぞれの横にレポート（.report.txt）を書く。
#[tauri::command]
pub fn export_start(state: State<'_, Shared>, jobs: Vec<Job>, report: bool) -> Result<(), String> {
    let mut s = lock(&state);
    let engine = s.engine.as_ref().ok_or("プレビューを準備中です")?;
    let (project, footage, clips, dir) = (s.project.clone(), engine.footage.clone(), engine.clips.clone(), base_dir(s.path.as_deref()));
    let label = if jobs.len() == 1 { "書き出し中".to_string() } else { format!("キューを書き出し中（{} 件）", jobs.len()) };
    spawn(&mut s, label, "export", move |progress| {
        let exprs = znd_script::ExprEngine::new();
        let ectx = ExportContext {
            project: &project,
            footage: &*footage,
            clips: Some(&*clips),
            resolver: Some(&exprs),
            base_dir: Some(dir),
            accel: znd_preview::gpu(),
            composite: znd_preview::gpu_composite(),
        };
        let mut last = None;
        let mut failures = Vec::new();
        for job in &jobs {
            let comp = CompId::new(job.comp.as_str());
            let path = PathBuf::from(&job.path);
            match znd_media::export(&ectx, &comp, &path, &job.settings.settings(), &mut *progress) {
                Ok(r) => {
                    if report {
                        if let Some(c) = project.comp(&comp) {
                            let _ = std::fs::write(path.with_extension("report.txt"), r.to_text(c));
                        }
                    }
                    last = Some(r);
                }
                Err(znd_media::ExportError::Cancelled) => return Err("書き出しを中断しました".into()),
                Err(e) => failures.push(format!("{}: {e}", path.display())),
            }
        }
        match (last, failures.is_empty()) {
            (Some(r), true) if r.warnings.is_empty() => Ok((format!("書き出しました（{} フレーム）", r.frames), None)),
            (Some(r), true) => Ok((format!("書き出しました（{} フレーム）— 注意: {}", r.frames, r.warnings.join(" / ")), None)),
            (_, false) => Err(format!("書き出し失敗: {}", failures.join(" / "))),
            (None, true) => Err("キューが空です".into()),
        }
    })
}

/// 提出前チェック: 構造・権利・欠落した素材とフォント・エクスプレッション・解像度・光過敏性・ラウドネス。
#[tauri::command]
pub fn check_start(state: State<'_, Shared>, comp: String) -> Result<(), String> {
    let mut s = lock(&state);
    let engine = s.engine.as_ref().ok_or("プレビューを準備中です")?;
    let (project, footage, clips, dir) = (s.project.clone(), engine.footage.clone(), engine.clips.clone(), base_dir(s.path.as_deref()));
    let comp_id = CompId::new(comp);
    spawn(&mut s, "提出前チェック".into(), "check", move |progress| {
        let comp = project.comp(&comp_id).ok_or("コンポがありません")?;
        let mut out = Check::default();
        let mut section = |title: &str, lines: Vec<String>| {
            if lines.is_empty() {
                out.lines.push((true, format!("{title}: 問題なし")));
            } else {
                lines.into_iter().for_each(|l| out.lines.push((false, format!("{title}: {l}"))));
            }
        };
        section("構造", project.validate());
        section("権利", rights::export_warnings(&project, &comp_id));
        section(
            "欠落フッテージ",
            znd_media::manage::missing_footage(&project, &dir).iter().filter_map(|id| project.footage_item(id)).map(|f| f.path.clone()).collect(),
        );
        section("欠落フォント", znd_media::manage::missing_fonts(&project));
        let exprs = znd_script::ExprEngine::new();
        section("エクスプレッション", exprs.check(&project, &comp_id, Time::ZERO).iter().map(|e| e.to_string()).collect());
        section(
            "解像度",
            if (comp.width, comp.height) == (1920, 1080) { vec![] } else { vec![format!("{}x{}（上映提出は 1920x1080 が基本）", comp.width, comp.height)] },
        );
        let ectx = ExportContext {
            project: &project,
            footage: &*footage,
            clips: Some(&*clips),
            resolver: Some(&exprs),
            base_dir: Some(dir.clone()),
            accel: znd_preview::gpu(),
            composite: znd_preview::gpu_composite(),
        };
        let flashes = znd_media::review::scan_flashes(&ectx, &comp_id, &mut *progress).map_err(|e| e.to_string())?;
        let fr = comp.frame_rate;
        section(
            "光過敏性",
            flashes
                .iter()
                .map(|w| format!("{:.2}〜{:.2} 秒 {}", fr.frame_to_time(w.start_frame).seconds(), fr.frame_to_time(w.end_frame).seconds(), w.message()))
                .collect(),
        );
        let mixer = znd_audio::Mixer::new(&project, &*clips);
        let mixed = mixer.mix(&comp_id, 0, mixer.comp_frames(comp))?;
        if mixed.iter().any(|v| *v != 0.0) {
            let l = znd_audio::loudness::analyze(&mixed);
            let lufs = l.integrated_lufs.map(|v| format!("{v:.1} LUFS")).unwrap_or_else(|| "計測不能".into());
            out.lines.push((true, format!("ラウドネス {lufs} / トゥルーピーク {:.1} dBTP", l.true_peak_db)));
            let warnings = l.warnings(-16.0, -1.0);
            if warnings.is_empty() {
                out.lines.push((true, "音量: 問題なし".into()));
            } else {
                warnings.into_iter().for_each(|w| out.lines.push((false, format!("音量: {w}"))));
            }
        }
        out.flashes = flashes.iter().map(|w| (w.start_frame, w.end_frame, w.message())).collect();
        let problems = out.lines.iter().filter(|l| !l.0).count();
        let message = if problems == 0 { "提出前チェック: 問題は見つかりませんでした".into() } else { format!("提出前チェック: {problems} 件の指摘があります") };
        Ok((message, Some(out)))
    })
}

/// 最後の提出前チェックの結果。
#[tauri::command]
pub fn check_result(state: State<'_, Shared>) -> Option<Check> {
    lock(&state).check.clone()
}

/// メイキング用の分解（レイヤーを順に重ねていく途中の画像）を、選んだフォルダに書き出す。
#[tauri::command]
pub fn export_breakdown(state: State<'_, Shared>, comp: String, frame: i64) -> Option<String> {
    let dir = rfd::FileDialog::new().pick_folder()?;
    let s = lock(&state);
    let engine = s.engine.as_ref()?;
    let id = CompId::new(comp);
    let t = s.project.comp(&id)?.frame_rate.frame_to_time(frame);
    let exprs = znd_script::ExprEngine::new();
    let ctx = ExportContext {
        project: &s.project,
        footage: &*engine.footage,
        clips: None,
        resolver: Some(&exprs),
        base_dir: Some(base_dir(s.path.as_deref())),
        accel: znd_preview::gpu(),
        composite: znd_preview::gpu_composite(),
    };
    Some(match znd_media::review::export_breakdown(&ctx, &id, t, &dir, "making") {
        Ok(files) => format!("{} 枚の分解画像を {} に書き出しました", files.len(), dir.display()),
        Err(e) => format!("書き出せません: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ui(format: usize) -> ExportUi {
        ExportUi { format, crf: 14, kbps: 8000, audio: true, pad: 1.0, transparent: false, range: Some((3, 9)), frame: 5, values: HashMap::new() }
    }

    #[test]
    fn export_settings_follow_the_chosen_format() {
        assert_eq!(ui(0).settings().format.extension(), "mp4");
        assert_eq!(ui(4).settings().format.extension(), "mov");
        let s = ui(0).settings();
        assert_eq!((s.pad_head, s.pad_tail, s.range), (1.0, 1.0, Some((3, 9))));
        assert_eq!(ui(9).settings().downsample, 2);
        assert_eq!(export_formats().len(), FORMATS.len() + hooks::exporters().len());
    }
}
