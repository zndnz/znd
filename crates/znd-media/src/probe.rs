//! メディアファイルの情報取得とフッテージ登録。

use crate::{tools, MediaError};
use serde::Deserialize;
use std::path::Path;
use std::process::Command;
use znd_core::{AudioInfo, FootageId, FootageItem, FrameRate, Project, Rights, Time};

#[derive(Debug, Clone, PartialEq)]
pub struct MediaInfo {
    /// 映像が無いファイル（音声のみ・3D モデル）は 0。
    pub width: u32,
    pub height: u32,
    /// 静止画・音声のみは `None`。
    pub frame_rate: Option<FrameRate>,
    pub duration: Option<Time>,
    pub frame_count: Option<i64>,
    pub codec: String,
    /// 可変フレームレート。読み込み時に `frame_rate` の固定レートへ適合させる。
    pub maybe_vfr: bool,
    pub audio: Option<AudioInfo>,
    /// 読み込み時に知らせるべき注意（音ズレの疑いなど）。
    pub warnings: Vec<String>,
}

#[derive(Deserialize)]
struct ProbeOut {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}

#[derive(Deserialize)]
struct ProbeStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    r_frame_rate: Option<String>,
    avg_frame_rate: Option<String>,
    nb_frames: Option<String>,
    nb_read_frames: Option<String>,
    duration: Option<String>,
    sample_rate: Option<String>,
    channels: Option<u32>,
    #[serde(default)]
    disposition: std::collections::HashMap<String, i32>,
}

#[derive(Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
}

const STILL_EXTENSIONS: [&str; 9] = ["png", "jpg", "jpeg", "exr", "tif", "tiff", "tga", "bmp", "webp"];
const MODEL_EXTENSIONS: [&str; 5] = ["obj", "gltf", "glb", "fbx", "abc"];

fn ext(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

pub fn is_still(path: &Path) -> bool {
    STILL_EXTENSIONS.contains(&ext(path).as_str()) && !is_apng(path) && !crate::sequence::is_pattern(path)
}

/// アニメーション PNG（`acTL` チャンクが画像データより前にある PNG）か。
pub fn is_apng(path: &Path) -> bool {
    use std::io::Read;
    let e = ext(path);
    if e == "apng" {
        return true;
    }
    if e != "png" {
        return false;
    }
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let mut head = vec![0u8; 4096];
    let n = f.read(&mut head).unwrap_or(0);
    let head = &head[..n];
    let find = |tag: &[u8]| head.windows(4).position(|w| w == tag);
    match (find(b"acTL"), find(b"IDAT")) {
        (Some(a), Some(i)) => a < i,
        (Some(_), None) => true,
        _ => false,
    }
}

/// ffmpeg / ffprobe に入力形式を明示する必要があるファイル用の引数（`-i` の前に置く）。
pub fn input_format_args(path: &Path) -> &'static [&'static str] {
    if is_apng(path) {
        &["-f", "apng"]
    } else {
        &[]
    }
}

pub fn is_model(path: &Path) -> bool {
    MODEL_EXTENSIONS.contains(&ext(path).as_str())
}

fn parse_rate(s: &str) -> Option<FrameRate> {
    let (n, d) = s.split_once('/')?;
    let (num, den): (u32, u32) = (n.parse().ok()?, d.parse().ok()?);
    (num > 0 && den > 0).then_some(FrameRate { num, den })
}

/// 可変フレームレート素材を適合させる先の固定レート。平均レートに最も近いプリセット。
fn conform_rate(average: FrameRate) -> FrameRate {
    FrameRate::PRESETS
        .into_iter()
        .min_by(|a, b| (a.fps() - average.fps()).abs().total_cmp(&(b.fps() - average.fps()).abs()))
        .unwrap_or(average)
}

/// 先頭 10 秒のフレーム時刻を調べ、間隔が不揃いなら実測の平均フレームレートを返す。
fn measure_irregular_rate(path: &Path) -> Option<FrameRate> {
    let out = Command::new(tools::ffprobe().ok()?)
        .args(["-v", "error", "-select_streams", "v:0", "-read_intervals", "%+10", "-show_entries", "packet=pts_time", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .ok()?;
    let mut pts: Vec<f64> =
        String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.trim().trim_end_matches(',').parse().ok()).collect();
    if pts.len() < 8 {
        return None;
    }
    pts.sort_by(|a, b| a.total_cmp(b));
    let gaps: Vec<f64> = pts.windows(2).map(|w| w[1] - w[0]).collect();
    let mut sorted = gaps.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let median = sorted[sorted.len() / 2];
    // 中央値から 50% 以上外れた間隔が全体の 2% を超えたら可変とみなす（タイムスタンプの丸め誤差では反応しない）
    let odd = gaps.iter().filter(|g| (**g - median).abs() > median * 0.5).count();
    if median <= 0.0 || (odd as f64) < gaps.len() as f64 * 0.02 {
        return None;
    }
    let fps = (pts.len() - 1) as f64 / (pts[pts.len() - 1] - pts[0]);
    Some(FrameRate { num: (fps * 1000.0).round() as u32, den: 1000 })
}

/// 動画・静止画・音声・3D モデルの情報を返す。`count_frames` が true なら実際にデコードしてフレーム数を数える（遅い）。
pub fn probe_with(path: &Path, count_frames: bool) -> Result<MediaInfo, MediaError> {
    let file_err = |message: String| MediaError::File { path: path.display().to_string(), message };
    let empty = |codec: &str| MediaInfo {
        width: 0,
        height: 0,
        frame_rate: None,
        duration: None,
        frame_count: None,
        codec: codec.into(),
        maybe_vfr: false,
        audio: None,
        warnings: Vec::new(),
    };
    if is_model(path) {
        if !path.exists() {
            return Err(file_err("ファイルがありません".into()));
        }
        return Ok(empty("model"));
    }
    if crate::sequence::is_pattern(path) {
        let seq = crate::sequence::scan(path).map_err(file_err)?;
        let first = crate::sequence::frame_path(path, seq.start);
        let (width, height) = image::image_dimensions(&first).map_err(|e| file_err(e.to_string()))?;
        let rate = crate::sequence::DEFAULT_RATE;
        let mut info = MediaInfo {
            width,
            height,
            frame_rate: Some(rate),
            duration: Some(rate.frame_to_time(seq.count)),
            frame_count: Some(seq.count),
            ..empty("sequence")
        };
        if seq.missing > 0 {
            info.warnings.push(format!("連番に {} 枚の抜けがあります。抜けたコマは直前のコマで埋めます", seq.missing));
        }
        return Ok(info);
    }
    if is_still(path) {
        let (width, height) = image::image_dimensions(path).map_err(|e| file_err(e.to_string()))?;
        return Ok(MediaInfo { width, height, ..empty("still") });
    }
    let mut cmd = Command::new(tools::ffprobe()?);
    cmd.args(["-v", "error", "-show_streams", "-show_format", "-of", "json"]);
    cmd.args(input_format_args(path));
    if count_frames {
        cmd.args(["-count_frames", "-select_streams", "v:0"]);
    }
    let out = cmd.arg(path).output().map_err(|e| MediaError::Tool { tool: "ffprobe", message: e.to_string() })?;
    if !out.status.success() {
        return Err(file_err(String::from_utf8_lossy(&out.stderr).trim().to_string()));
    }
    let parsed: ProbeOut = serde_json::from_slice(&out.stdout).map_err(|e| file_err(e.to_string()))?;
    let format_seconds = parsed.format.and_then(|f| f.duration).and_then(|s| s.parse::<f64>().ok());
    let seconds_of = |s: &ProbeStream| s.duration.as_deref().and_then(|d| d.parse::<f64>().ok());
    // カバーアート（attached_pic）は映像として扱わない
    let video = parsed
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video") && s.disposition.get("attached_pic").copied().unwrap_or(0) == 0);
    let audio_stream = parsed.streams.iter().find(|s| s.codec_type.as_deref() == Some("audio"));
    let audio = audio_stream.and_then(|s| {
        let seconds = seconds_of(s).or(format_seconds)?;
        Some(AudioInfo {
            sample_rate: s.sample_rate.as_deref()?.parse().ok()?,
            channels: s.channels?,
            duration: Time::from_seconds(seconds),
        })
    });

    let Some(stream) = video else {
        if audio.is_none() {
            return Err(file_err("映像・音声ストリームがありません".into()));
        }
        let codec = audio_stream.and_then(|s| s.codec_name.clone()).unwrap_or_default();
        return Ok(MediaInfo { duration: audio.map(|a| a.duration), audio, ..empty(&codec) });
    };
    let (Some(width), Some(height)) = (stream.width, stream.height) else {
        return Err(file_err("解像度を取得できません".into()));
    };
    let nominal = stream.r_frame_rate.as_deref().and_then(parse_rate);
    let average = stream.avg_frame_rate.as_deref().and_then(parse_rate);
    let mut maybe_vfr = match (nominal, average) {
        (Some(n), Some(a)) => (n.fps() - a.fps()).abs() / n.fps() > 0.001,
        _ => false,
    };
    let mut average = average;
    if !maybe_vfr {
        // コンテナによっては平均レートが正しく記録されない（MKV 等）ので、実際のフレーム間隔も確かめる
        if let Some(measured) = measure_irregular_rate(path) {
            maybe_vfr = true;
            average = Some(measured);
        }
    }
    let mut warnings = Vec::new();
    let frame_rate = if maybe_vfr {
        let target = conform_rate(average.or(nominal).ok_or_else(|| file_err("フレームレートを取得できません".into()))?);
        warnings.push(format!("可変フレームレート素材です。{:.3}fps の固定レートに適合させて読み込みます", target.fps()));
        target
    } else {
        nominal.or(average).ok_or_else(|| file_err("フレームレートを取得できません".into()))?
    };
    let seconds = seconds_of(stream).or(format_seconds);
    if seconds.is_none() && stream.nb_frames.is_none() && stream.nb_read_frames.is_none() && !count_frames {
        // 尺が記録されていない形式（APNG 等）は、実際にデコードして数える
        return probe_with(path, true);
    }
    let frame_count = if maybe_vfr {
        seconds.map(|s| (s * frame_rate.fps()).round() as i64)
    } else {
        stream
            .nb_read_frames
            .clone()
            .or(stream.nb_frames.clone())
            .and_then(|s| s.parse::<i64>().ok())
            .or_else(|| seconds.map(|s| (s * frame_rate.fps()).round() as i64))
    };
    // 尺はフレーム数から求める（コンテナの秒数は丸められていることがある）
    let duration = frame_count.map(|n| frame_rate.frame_to_time(n)).or_else(|| seconds.map(Time::from_seconds));
    // 映像と音声の尺が 1 フレーム以上違えば、音ズレの疑いとして知らせる
    if let (Some(v), Some(a)) = (duration, audio) {
        let diff = (v.seconds() - a.duration.seconds()).abs();
        if diff > 1.0 / frame_rate.fps() {
            warnings.push(format!("映像と音声の尺が {:.3} 秒ずれています。音ズレがないか確認してください", diff));
        }
    }
    Ok(MediaInfo {
        width,
        height,
        frame_rate: Some(frame_rate),
        duration,
        frame_count,
        codec: stream.codec_name.clone().unwrap_or_default(),
        maybe_vfr,
        audio,
        warnings,
    })
}

pub fn probe(path: &Path) -> Result<MediaInfo, MediaError> {
    probe_with(path, false)
}

/// ファイルを調べてプロジェクトにフッテージとして登録する。権利メタデータは「未確認」で始まる。
/// 読み込み時の注意（VFR・音ズレ）があれば一緒に返す。
pub fn import_footage_with_warnings(project: &mut Project, path: &Path) -> Result<(FootageId, Vec<String>), MediaError> {
    let info = probe(path)?;
    let id = FootageId::new(project.fresh_id("footage"));
    project.footage.push(FootageItem {
        id: id.clone(),
        name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        path: path.display().to_string(),
        width: info.width,
        height: info.height,
        frame_rate: info.frame_rate,
        duration: info.duration,
        rights: Rights::default(),
        audio: info.audio,
        vfr: info.maybe_vfr,
    });
    Ok((id, info.warnings))
}

pub fn import_footage(project: &mut Project, path: &Path) -> Result<FootageId, MediaError> {
    import_footage_with_warnings(project, path).map(|(id, _)| id)
}

/// 連番画像を 1 本のフッテージとして登録する。`file` は連番のうちの 1 枚、または `%04d` 形式の書式。
pub fn import_sequence(project: &mut Project, file: &Path, frame_rate: FrameRate) -> Result<(FootageId, Vec<String>), MediaError> {
    let pattern = if crate::sequence::is_pattern(file) {
        file.to_path_buf()
    } else {
        crate::sequence::pattern_from_file(file).ok_or_else(|| MediaError::File {
            path: file.display().to_string(),
            message: "連番のファイルが見つかりません（名前の末尾が数字で、同じ規則のファイルが 2 枚以上必要です）".into(),
        })?
    };
    let (id, warnings) = import_footage_with_warnings(project, &pattern)?;
    if let Some(item) = project.footage.iter_mut().find(|f| f.id == id) {
        let frames = item.frame_rate.zip(item.duration).map(|(r, d)| r.frame_count(d)).unwrap_or(0);
        item.frame_rate = Some(frame_rate);
        item.duration = Some(frame_rate.frame_to_time(frames));
    }
    Ok((id, warnings))
}
