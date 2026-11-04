//! コンポジションの書き出し。動画（H.264 / H.265 / ProRes / GIF）と連番（PNG / EXR）。

use crate::audio::write_wav;
use crate::encode::{run_ffmpeg, H264Settings, PipeFormat, VideoEncoder};
use crate::MediaError;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use znd_audio::loudness::{analyze, LoudnessReport};
use znd_audio::{time_to_sample, ClipSource, Mixer, SAMPLE_RATE};
use znd_core::{rights, ColorSpace, CompId, Composition, Project};
use znd_render::{
    encode_rgb8, encode_rgba16, encode_rgba8, linear_output, to_rgba8, FootageSource, FrameBuf, OutputXf, RenderError, RenderOptions,
    Renderer, Resolver,
};

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error(transparent)]
    Render(#[from] RenderError),
    #[error(transparent)]
    Media(#[from] MediaError),
    #[error("書き出しを中断しました")]
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Format {
    H264 {
        crf: u8,
        preset: String,
    },
    /// `two_pass_kbps` を指定すると、目標ビットレートの 2 パスエンコードになる。
    H265 {
        crf: u8,
        preset: String,
        two_pass_kbps: Option<u32>,
    },
    ProRes422,
    /// アルファ付き。
    ProRes4444,
    PngSequence {
        sixteen_bit: bool,
    },
    /// リニアの 32bit float EXR。
    ExrSequence,
    Gif,
}

impl Format {
    pub fn is_sequence(&self) -> bool {
        matches!(self, Format::PngSequence { .. } | Format::ExrSequence)
    }
    pub fn has_alpha(&self) -> bool {
        matches!(self, Format::ProRes4444 | Format::PngSequence { .. } | Format::ExrSequence)
    }
    pub fn supports_audio(&self) -> bool {
        matches!(self, Format::H264 { .. } | Format::H265 { .. } | Format::ProRes422 | Format::ProRes4444)
    }
    /// 4:2:0 / 4:2:2 のクロマサブサンプリングのため偶数寸法が必要か。
    fn needs_even(&self) -> bool {
        matches!(self, Format::H264 { .. } | Format::H265 { .. } | Format::ProRes422)
    }
    pub fn extension(&self) -> &'static str {
        match self {
            Format::H264 { .. } | Format::H265 { .. } => "mp4",
            Format::ProRes422 | Format::ProRes4444 => "mov",
            Format::PngSequence { .. } => "png",
            Format::ExrSequence => "exr",
            Format::Gif => "gif",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportSettings {
    pub format: Format,
    /// 書き出すフレーム範囲 [開始, 終了)。`None` で全尺。
    #[serde(default)]
    pub range: Option<(i64, i64)>,
    #[serde(default = "yes")]
    pub audio: bool,
    /// 冒頭・末尾に足す黒味（秒）。
    #[serde(default)]
    pub pad_head: f64,
    #[serde(default)]
    pub pad_tail: f64,
    /// 1/n 解像度で書き出す（GIF や確認用の小さい書き出し）。
    #[serde(default = "one")]
    pub downsample: u32,
    /// アルファ対応形式で、背景を敷かずに透過のまま書き出す。
    #[serde(default)]
    pub transparent: bool,
}

fn yes() -> bool {
    true
}
fn one() -> u32 {
    1
}

impl ExportSettings {
    pub fn new(format: Format) -> Self {
        ExportSettings { format, range: None, audio: true, pad_head: 0.0, pad_tail: 0.0, downsample: 1, transparent: false }
    }

    pub fn h264(s: &H264Settings) -> Self {
        Self::new(Format::H264 { crf: s.crf, preset: s.preset.clone() })
    }

    /// 上映提出用: 高画質 H.264、音声付き、冒頭・末尾に黒味。
    pub fn screening(pad_seconds: f64) -> Self {
        ExportSettings { pad_head: pad_seconds, pad_tail: pad_seconds, ..Self::new(Format::H264 { crf: 12, preset: "slow".into() }) }
    }

    /// エンディング・クレジット用の短尺素材: 指定範囲を 1/2 解像度で。
    pub fn credits_clip(start_frame: i64, end_frame: i64) -> Self {
        ExportSettings {
            range: Some((start_frame, end_frame)),
            downsample: 2,
            ..Self::new(Format::H264 { crf: 18, preset: "medium".into() })
        }
    }
}

pub struct ExportContext<'a> {
    pub project: &'a Project,
    pub footage: &'a dyn FootageSource,
    pub clips: Option<&'a dyn ClipSource>,
    pub resolver: Option<&'a dyn Resolver>,
    pub base_dir: Option<PathBuf>,
    /// エフェクトの高速化バックエンド（GPU）。`None` ならすべて CPU で処理する。
    pub accel: Option<&'a dyn znd_render::effects::EffectAccel>,
    /// 合成の高速化バックエンド（GPU）。`None` なら CPU で合成する。
    pub composite: Option<&'a dyn znd_render::gpu_composite::CompositeAccel>,
}

impl<'a> ExportContext<'a> {
    pub fn new(project: &'a Project, footage: &'a dyn FootageSource) -> Self {
        ExportContext { project, footage, clips: None, resolver: None, base_dir: None, accel: None, composite: None }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExportReport {
    pub frames: i64,
    pub width: usize,
    pub height: usize,
    pub loudness: Option<LoudnessReport>,
    /// 権利・音量などの注意。書き出し自体は成功している。
    pub warnings: Vec<String>,
}

impl ExportReport {
    /// 提出時に添える、人が読むレポート。
    pub fn to_text(&self, comp: &Composition) -> String {
        let mut s = format!(
            "コンポ: {}\n解像度: {}x{}\nフレームレート: {:.3} fps\nフレーム数: {}\n",
            comp.name,
            self.width,
            self.height,
            comp.frame_rate.fps(),
            self.frames
        );
        match &self.loudness {
            Some(l) => {
                match l.integrated_lufs {
                    Some(v) => s += &format!("統合ラウドネス: {v:.1} LUFS\n"),
                    None => s += "統合ラウドネス: 計測不能（無音または短すぎ）\n",
                }
                s += &format!("トゥルーピーク: {:.1} dBTP\nクリッピング: {} サンプル\n", l.true_peak_db, l.clipped_samples);
            }
            None => s += "音声: なし\n",
        }
        if self.warnings.is_empty() {
            s += "警告: なし\n";
        } else {
            s += "警告:\n";
            for w in &self.warnings {
                s += &format!("  - {w}\n");
            }
        }
        s
    }
}

/// 上映用途のラウドネスの目安。配信向け（-14 LUFS 前後）と同程度までを許容する。
const TARGET_LUFS: f64 = -16.0;
const MAX_TRUE_PEAK_DB: f64 = -1.0;

fn color_tags(space: ColorSpace) -> Vec<String> {
    let (prim, trc) = match space {
        ColorSpace::DisplayP3 => ("smpte432", "iec61966-2-1"),
        _ => ("bt709", "bt709"),
    };
    ["-color_range", "tv", "-colorspace", "bt709", "-color_primaries", prim, "-color_trc", trc].iter().map(|s| s.to_string()).collect()
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

const YUV: &str = "scale=in_range=pc:out_range=tv:out_color_matrix=bt709:flags=accurate_rnd+full_chroma_int";

/// 形式ごとの (パイプ形式, 出力引数)。
fn encoder_args(format: &Format, space: ColorSpace, has_audio: bool) -> (PipeFormat, Vec<String>) {
    let mut args = Vec::new();
    let pipe = match format {
        Format::H264 { crf, preset } => {
            args.extend(s(&["-vf", &format!("{YUV},format=yuv420p"), "-c:v", "libx264", "-preset", preset, "-crf", &crf.to_string()]));
            args.extend(color_tags(space));
            args.extend(s(&["-movflags", "+faststart"]));
            PipeFormat::Rgb8
        }
        Format::H265 { crf, preset, .. } => {
            // 10bit で符号化する（グラデーションのバンディング対策）
            args.extend(s(&["-vf", &format!("{YUV},format=yuv420p10le"), "-c:v", "libx265", "-preset", preset, "-crf", &crf.to_string()]));
            args.extend(s(&["-x265-params", "log-level=error", "-tag:v", "hvc1"]));
            args.extend(color_tags(space));
            args.extend(s(&["-movflags", "+faststart"]));
            PipeFormat::Rgba16
        }
        Format::ProRes422 => {
            args.extend(s(&["-vf", &format!("{YUV},format=yuv422p10le"), "-c:v", "prores_ks", "-profile:v", "3", "-vendor", "apl0"]));
            args.extend(color_tags(space));
            PipeFormat::Rgba16
        }
        Format::ProRes4444 => {
            args.extend(s(&["-vf", &format!("{YUV},format=yuva444p10le"), "-c:v", "prores_ks", "-profile:v", "4", "-vendor", "apl0"]));
            args.extend(color_tags(space));
            PipeFormat::Rgba16
        }
        Format::Gif => {
            args.extend(s(&["-vf", "split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=sierra2_4a", "-loop", "0"]));
            PipeFormat::Rgb8
        }
        Format::PngSequence { .. } | Format::ExrSequence => unreachable!("sequences are written directly"),
    };
    if has_audio {
        let codec: &[&str] = if matches!(format, Format::ProRes422 | Format::ProRes4444) {
            &["-c:a", "pcm_s24le"]
        } else {
            &["-c:a", "aac", "-b:a", "320k"]
        };
        args.extend(s(codec));
        args.extend(s(&["-map", "0:v:0", "-map", "1:a:0"]));
    }
    (pipe, args)
}

fn frame_bytes(buf: &FrameBuf, pipe: PipeFormat, frame: i64, xf: &OutputXf) -> Vec<u8> {
    match pipe {
        PipeFormat::Rgb8 => encode_rgb8(buf, Some(frame as u32), xf),
        PipeFormat::Rgba16 => encode_rgba16(buf, xf).iter().flat_map(|v| v.to_le_bytes()).collect(),
    }
}

fn file_err(path: &Path, e: impl std::fmt::Display) -> MediaError {
    MediaError::File { path: path.display().to_string(), message: e.to_string() }
}

/// 連番ファイル名: `shot.png` → `shot_00012.png`。
pub fn sequence_path(path: &Path, frame: i64) -> PathBuf {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "frame".into());
    let ext = path.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!("{stem}_{frame:05}.{ext}"))
}

fn write_still(buf: &FrameBuf, path: &Path, format: &Format, frame: i64, xf: &OutputXf) -> Result<(), MediaError> {
    let (w, h) = (buf.width as u32, buf.height as u32);
    match format {
        Format::PngSequence { sixteen_bit: false } => {
            image::save_buffer(path, &encode_rgba8(buf, Some(frame as u32), xf), w, h, image::ExtendedColorType::Rgba8)
        }
        Format::PngSequence { sixteen_bit: true } => {
            // image クレートはネイティブエンディアンの u16 列を受け取り、PNG のビッグエンディアンへは内部で変換する
            let bytes: Vec<u8> = encode_rgba16(buf, xf).iter().flat_map(|v| v.to_ne_bytes()).collect();
            image::save_buffer(path, &bytes, w, h, image::ExtendedColorType::Rgba16)
        }
        _ => {
            let bytes: Vec<u8> = linear_output(buf, xf).iter().flat_map(|v| v.to_ne_bytes()).collect();
            image::save_buffer(path, &bytes, w, h, image::ExtendedColorType::Rgba32F)
        }
    }
    .map_err(|e| file_err(path, e))
}

/// コンポを書き出す。
///
/// 複数フレームを並列にレンダリングし、順番どおりにエンコーダへ渡す。
/// `progress(完了フレーム数, 総フレーム数)` が false を返すと中断し、書きかけのファイルを削除する。
pub fn export(
    ctx: &ExportContext,
    comp_id: &CompId,
    path: &Path,
    settings: &ExportSettings,
    mut progress: impl FnMut(i64, i64) -> bool,
) -> Result<ExportReport, ExportError> {
    let project = ctx.project;
    let comp = project.comp(comp_id).ok_or_else(|| RenderError::MissingComp(comp_id.clone()))?;
    let total_comp = comp.frame_count();
    let (first, last) = settings.range.map(|(a, b)| (a.clamp(0, total_comp), b.clamp(0, total_comp))).unwrap_or((0, total_comp));
    if last <= first {
        return Err(MediaError::Invalid("書き出す範囲にフレームがありません".into()).into());
    }
    let fr = comp.frame_rate;
    let (head, tail) = ((settings.pad_head * fr.fps()).round().max(0.0) as i64, (settings.pad_tail * fr.fps()).round().max(0.0) as i64);
    let total = head + (last - first) + tail;
    let (w, h) = Renderer::output_size(comp, settings.downsample);
    if settings.format.needs_even() && (w % 2 != 0 || h % 2 != 0) {
        return Err(MediaError::Invalid(format!("この形式は偶数の解像度が必要です（{w}x{h}）。コンポの解像度を変更してください")).into());
    }

    let mut warnings = rights::export_warnings(project, comp_id);
    let mixer = ctx.clips.map(|c| Mixer::new(project, c));
    let mut renderer = Renderer::new(project, ctx.footage).with_accel(ctx.accel).with_composite(ctx.composite);
    if let Some(r) = ctx.resolver {
        renderer = renderer.with_resolver(r);
    }
    if let Some(m) = &mixer {
        renderer = renderer.with_audio(m);
    }
    if let Some(d) = &ctx.base_dir {
        renderer = renderer.with_base_dir(d.clone());
    }
    let xf = renderer.output_xf();
    let transparent = settings.transparent && settings.format.has_alpha();
    let opts = RenderOptions { downsample: settings.downsample.max(1), background: !transparent, guides: false };

    // 音声: 範囲をミックスし、黒味ぶんの無音を足して一時 WAV にする
    let mut loudness = None;
    let mut wav: Option<PathBuf> = None;
    if let (true, Some(m)) = (settings.audio, &mixer) {
        let (a, b) = (time_to_sample(fr.frame_to_time(first)), time_to_sample(fr.frame_to_time(last)));
        let mixed = m.mix(comp_id, a, (b - a) as usize).map_err(MediaError::Invalid)?;
        if mixed.iter().any(|v| *v != 0.0) {
            let pad = |frames: i64| vec![0.0f32; (time_to_sample(fr.frame_to_time(frames)) * 2) as usize];
            let mut full = pad(head);
            full.extend(&mixed);
            full.extend(pad(tail));
            let report = analyze(&mixed);
            warnings.extend(report.warnings(TARGET_LUFS, MAX_TRUE_PEAK_DB));
            loudness = Some(report);
            if settings.format.supports_audio() {
                let p = path.with_extension("znd-audio.wav");
                write_wav(&p, &full)?;
                wav = Some(p);
            }
        }
    }
    let _ = SAMPLE_RATE;

    let black =
        FrameBuf { width: w, height: h, data: if transparent { vec![0.0; w * h * 4] } else { [0.0f32, 0.0, 0.0, 1.0].repeat(w * h) } };
    // 出力フレーム番号 → コンポのフレーム番号（黒味は None）
    let source_of = |out: i64| -> Option<i64> { (out >= head && out < head + (last - first)).then(|| first + out - head) };
    let render = |out: i64| -> Result<FrameBuf, RenderError> {
        match source_of(out) {
            Some(f) => renderer.render_frame(comp_id, fr.frame_to_time(f), opts),
            None => Ok(black.clone()),
        }
    };
    let batch = rayon::current_num_threads().max(1) as i64;

    // 2 パス H.265 は、いったん可逆の中間ファイルに書いてから 2 回エンコードする
    let two_pass = match &settings.format {
        Format::H265 { two_pass_kbps: Some(kbps), preset, .. } => Some((*kbps, preset.clone())),
        _ => None,
    };
    let intermediate = two_pass.as_ref().map(|_| path.with_extension("znd-intermediate.mkv"));
    let mut cleanup: Vec<PathBuf> = wav.iter().cloned().chain(intermediate.iter().cloned()).collect();

    let result: Result<(), ExportError> = (|| {
        if settings.format.is_sequence() {
            let mut done = 0;
            while done < total {
                let frames: Vec<i64> = (done..(done + batch).min(total)).collect();
                let results: Vec<Result<(), ExportError>> = frames
                    .par_iter()
                    .map(|f| {
                        let buf = render(*f)?;
                        // 連番の番号はコンポのフレーム番号に合わせる（範囲書き出しでも他ツールと位置が合う）
                        Ok(write_still(&buf, &sequence_path(path, first - head + *f), &settings.format, *f, &xf)?)
                    })
                    .collect();
                for r in results {
                    r?;
                    done += 1;
                }
                if !progress(done, total) {
                    return Err(ExportError::Cancelled);
                }
            }
            return Ok(());
        }

        let (pipe, mut args) = encoder_args(&settings.format, project.settings.output_space, wav.is_some() && two_pass.is_none());
        let target = match &intermediate {
            Some(tmp) => {
                args = s(&["-c:v", "ffv1", "-level", "3", "-pix_fmt", "gbrp16le"]);
                tmp.clone()
            }
            None => path.to_path_buf(),
        };
        let audio_input = if two_pass.is_none() { wav.as_deref() } else { None };
        let mut encoder = VideoEncoder::spawn(pipe, w, h, fr, audio_input, &args, &target)?;
        let mut done = 0;
        while done < total {
            let frames: Vec<i64> = (done..(done + batch).min(total)).collect();
            let rendered: Vec<Result<Vec<u8>, RenderError>> =
                frames.par_iter().map(|f| Ok(frame_bytes(&render(*f)?, pipe, *f, &xf))).collect();
            for bytes in rendered {
                encoder.write_frame(&bytes?)?;
                done += 1;
            }
            if !progress(done, total) {
                return Err(ExportError::Cancelled);
            }
        }
        encoder.finish()?;

        if let (Some((kbps, preset)), Some(tmp)) = (&two_pass, &intermediate) {
            let stats = path.with_extension("znd-x265.log");
            cleanup.push(stats.clone());
            cleanup.push(PathBuf::from(format!("{}.cutree", stats.display())));
            let common = |pass: u32| -> Vec<String> {
                let mut a = s(&["-i", &tmp.display().to_string()]);
                if pass == 2 {
                    if let Some(wv) = &wav {
                        a.extend(s(&["-i", &wv.display().to_string()]));
                    }
                }
                a.extend(s(&[
                    "-vf",
                    &format!("{YUV},format=yuv420p10le"),
                    "-c:v",
                    "libx265",
                    "-preset",
                    preset,
                    "-b:v",
                    &format!("{kbps}k"),
                ]));
                a.extend(s(&["-x265-params", &format!("log-level=error:pass={pass}:stats={}", stats.display()), "-tag:v", "hvc1"]));
                a.extend(color_tags(project.settings.output_space));
                a
            };
            let mut p1 = common(1);
            p1.extend(s(&["-an", "-f", "null", if cfg!(windows) { "NUL" } else { "/dev/null" }]));
            run_ffmpeg(&p1)?;
            let mut p2 = common(2);
            if wav.is_some() {
                p2.extend(s(&["-c:a", "aac", "-b:a", "320k", "-map", "0:v:0", "-map", "1:a:0"]));
            }
            p2.extend(s(&["-movflags", "+faststart", &path.display().to_string()]));
            run_ffmpeg(&p2)?;
        }
        Ok(())
    })();

    for f in &cleanup {
        let _ = std::fs::remove_file(f);
    }
    match result {
        Ok(()) => Ok(ExportReport { frames: total, width: w, height: h, loudness, warnings }),
        Err(e) => {
            if !settings.format.is_sequence() {
                let _ = std::fs::remove_file(path);
            }
            Err(e)
        }
    }
}

/// コンポ全尺を H.264 (mp4) に書き出す（映像のみ）。
pub fn export_h264(
    project: &Project,
    comp_id: &CompId,
    footage: &dyn FootageSource,
    path: &Path,
    settings: &H264Settings,
    progress: impl FnMut(i64, i64) -> bool,
) -> Result<(), ExportError> {
    export(
        &ExportContext::new(project, footage),
        comp_id,
        path,
        &ExportSettings { audio: false, ..ExportSettings::h264(settings) },
        progress,
    )
    .map(|_| ())
}

/// アルファ付き 8bit sRGB PNG として保存する。
pub fn save_png(buf: &FrameBuf, path: &Path, dither_seed: Option<u32>) -> Result<(), MediaError> {
    image::save_buffer(path, &to_rgba8(buf, dither_seed), buf.width as u32, buf.height as u32, image::ExtendedColorType::Rgba8)
        .map_err(|e| file_err(path, e))
}

/// レンダーキューの 1 項目。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueueJob {
    pub comp: CompId,
    pub path: PathBuf,
    pub settings: ExportSettings,
}

#[derive(Debug, Clone, PartialEq)]
pub enum QueueEvent {
    Started { index: usize },
    Progress { index: usize, done: i64, total: i64 },
    Finished { index: usize, result: Result<ExportReport, String> },
}

/// キューを先頭から順に書き出す。1 つ失敗しても残りは続ける。`on_event` が false を返すと全体を中断する。
pub fn run_queue(
    ctx: &ExportContext,
    jobs: &[QueueJob],
    mut on_event: impl FnMut(QueueEvent) -> bool,
) -> Vec<Result<ExportReport, String>> {
    let mut results = Vec::new();
    for (index, job) in jobs.iter().enumerate() {
        if !on_event(QueueEvent::Started { index }) {
            break;
        }
        let mut keep_going = true;
        let r = export(ctx, &job.comp, &job.path, &job.settings, |done, total| {
            keep_going = on_event(QueueEvent::Progress { index, done, total });
            keep_going
        })
        .map_err(|e| e.to_string());
        on_event(QueueEvent::Finished { index, result: r.clone() });
        results.push(r);
        if !keep_going {
            break;
        }
    }
    results
}

/// OS の通知を出す（レンダー完了の知らせ）。失敗しても何もしない。
pub fn notify(title: &str, body: &str) {
    use std::process::{Command, Stdio};
    let quiet = |c: &mut Command| {
        let _ = c.stdout(Stdio::null()).stderr(Stdio::null()).status();
    };
    if cfg!(target_os = "macos") {
        let esc = |t: &str| t.replace('\\', "\\\\").replace('"', "\\\"");
        quiet(Command::new("osascript").args(["-e", &format!("display notification \"{}\" with title \"{}\"", esc(body), esc(title))]));
    } else if cfg!(target_os = "linux") {
        quiet(Command::new("notify-send").args([title, body]));
    } else {
        // Windows: 端末ベルで知らせる
        eprint!("\x07");
    }
}
