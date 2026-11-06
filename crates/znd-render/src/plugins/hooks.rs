//! プラグインからレンダリングへの介入点。
//!
//! - 出力フィルター: コンポの最終画像（背景を敷いた後）にエフェクトを重ねる。プレビューだけ・書き出しだけ・両方を選べる。
//!   プラグインのエフェクト（`plugin:` / `ofx:`）も指定できるので、ネイティブコードで出力全体を加工できる。
//! - エクスプレッション用ライブラリ: すべてのエクスプレッションから呼べる JavaScript の関数を追加する。
//!
//! どちらも変わるたびに世代番号が進み、プレビューのキャッシュの指紋に混ぜられる。

use crate::effects::{self, FxContext};
use crate::frame::FrameBuf;
use crate::ColorXf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use znd_core::effect::{EffectInstance, ParamValue};
use znd_core::property::Property;
use znd_core::{Composition, Time};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Preview,
    Export,
    Both,
}

impl Stage {
    pub fn parse(s: &str) -> Stage {
        match s {
            "preview" => Stage::Preview,
            "export" => Stage::Export,
            _ => Stage::Both,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Stage::Preview => "プレビューのみ",
            Stage::Export => "書き出しのみ",
            Stage::Both => "プレビューと書き出し",
        }
    }

    fn applies(self, preview: bool) -> bool {
        match self {
            Stage::Preview => preview,
            Stage::Export => !preview,
            Stage::Both => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FilterEffect {
    pub kind: String,
    pub params: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutputFilter {
    /// `パッケージID/フィルターID`
    pub key: String,
    pub plugin: String,
    pub label: String,
    pub stage: Stage,
    pub enabled: bool,
    pub effects: Vec<FilterEffect>,
}

fn filters() -> &'static Mutex<Vec<OutputFilter>> {
    static F: OnceLock<Mutex<Vec<OutputFilter>>> = OnceLock::new();
    F.get_or_init(Mutex::default)
}

fn libraries() -> &'static Mutex<Arc<Vec<(String, String)>>> {
    static L: OnceLock<Mutex<Arc<Vec<(String, String)>>>> = OnceLock::new();
    L.get_or_init(Mutex::default)
}

static GENERATION: AtomicU64 = AtomicU64::new(0);
static LIB_GENERATION: AtomicU64 = AtomicU64::new(0);

/// 出力フィルター・ライブラリが変わるたびに進む番号（キャッシュの指紋用）。
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

fn bump() {
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

pub fn register_filter(f: OutputFilter) {
    let mut list = filters().lock().unwrap();
    if let Some(old) = list.iter_mut().find(|o| o.key == f.key) {
        *old = f;
    } else {
        list.push(f);
    }
    bump();
}

pub fn output_filters() -> Vec<OutputFilter> {
    filters().lock().unwrap().clone()
}

pub fn set_filter_enabled(key: &str, on: bool) {
    let mut list = filters().lock().unwrap();
    if let Some(f) = list.iter_mut().find(|f| f.key == key) {
        if f.enabled != on {
            f.enabled = on;
            drop(list);
            bump();
        }
    }
}

/// 指紋に混ぜる値。有効な出力フィルターが無いときは 0（キャッシュを無駄に捨てない）。
pub(crate) fn fingerprint(preview: bool) -> u64 {
    let any = filters().lock().unwrap().iter().any(|f| f.enabled && f.stage.applies(preview));
    let libs = LIB_GENERATION.load(Ordering::Relaxed);
    if any {
        generation().wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ libs
    } else {
        libs
    }
}

/// 出力フィルターのエフェクトを作る。パラメータは既定値に JSON の値を上書きする。
pub fn build_effect(spec: &FilterEffect, w: f64, h: f64) -> Option<EffectInstance> {
    let mut fx = EffectInstance::new(&spec.kind, w, h)?;
    for (name, v) in &spec.params {
        let Some(slot) = fx.params.get_mut(name) else { continue };
        let nums: Vec<f64> = match v {
            serde_json::Value::Array(a) => a.iter().filter_map(|x| x.as_f64()).collect(),
            serde_json::Value::Number(n) => n.as_f64().into_iter().collect(),
            serde_json::Value::Bool(b) => vec![*b as u8 as f64],
            _ => Vec::new(),
        };
        match slot {
            ParamValue::Scalar(_) if !nums.is_empty() => *slot = ParamValue::Scalar(Property::constant(nums[0])),
            ParamValue::Point(_) if nums.len() >= 2 => *slot = ParamValue::Point(Property::constant([nums[0], nums[1]])),
            ParamValue::Color(_) if nums.len() >= 3 => {
                *slot = ParamValue::Color(Property::constant([nums[0], nums[1], nums[2], nums.get(3).copied().unwrap_or(1.0)]))
            }
            ParamValue::Bool(_) if !nums.is_empty() => *slot = ParamValue::Bool(nums[0] != 0.0),
            ParamValue::Choice(_) if !nums.is_empty() => *slot = ParamValue::Choice(nums[0].max(0.0) as u32),
            ParamValue::Text(_) => {
                if let Some(s) = v.as_str() {
                    *slot = ParamValue::Text(s.to_string());
                }
            }
            _ => {}
        }
    }
    Some(fx)
}

/// コンポの最終画像に出力フィルターを掛ける。
pub(crate) fn apply_output_filters(comp: &Composition, t: Time, mut buf: FrameBuf, scale: f32, cx: &ColorXf, preview: bool) -> FrameBuf {
    let active: Vec<OutputFilter> = filters().lock().unwrap().iter().filter(|f| f.enabled && f.stage.applies(preview)).cloned().collect();
    if active.is_empty() {
        return buf;
    }
    let none_layer = |_: &znd_core::LayerId| None;
    let none_time = |_: Time| None;
    let no_audio = |_: &znd_core::LayerId, _: usize| Vec::new();
    let seed = comp.id.0.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193));
    let (w, h) = (buf.width, buf.height);
    for f in &active {
        for spec in &f.effects {
            let Some(fx) = build_effect(spec, comp.width as f64, comp.height as f64) else { continue };
            let params = fx.eval(t);
            let eval_at = |at: Time| fx.eval(at);
            let offset = std::cell::Cell::new([0.0f64; 2]);
            let ctx = FxContext {
                time: t,
                frame_rate: comp.frame_rate,
                scale,
                cx,
                fetch_layer: &none_layer,
                fetch_self: &none_time,
                fetch_below: &|_, _| None,
                fetch_source: &|_| None,
                audio: &no_audio,
                eval_at: &eval_at,
                masks: &[],
                seed,
                offset: &offset,
                layer_time: t.seconds(),
                layer_duration: comp.duration.seconds(),
                comp_size: [comp.width, comp.height],
            };
            buf = effects::apply(&fx.kind, &params, buf, &ctx);
            // 出力フィルターでは画面の大きさを変えられないので、ずれた分を戻して元の大きさに貼り直す
            if buf.width != w || buf.height != h || offset.get() != [0.0, 0.0] {
                buf = crate::compositor::paste_at(&buf, offset.get(), w, h);
                offset.set([0.0, 0.0]);
            }
        }
    }
    buf
}

/// エクスプレッション用ライブラリ（名前, ソース）を追加する。同じ名前は置き換える。
/// プラグインが定義する書き出し形式のパラメータ（書き出し画面に並ぶ）。
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct ExporterParam {
    pub id: String,
    pub label: String,
    /// "number" / "choice" / "text" / "bool" / "args"（空白で区切った複数の引数。引数そのものが `{ID}` のときに展開する）
    #[serde(rename = "type", default = "default_param_type")]
    pub kind: String,
    /// 既定値（number は数値、choice は選択肢の番号、bool は真偽値、text は文字列）。
    #[serde(default)]
    pub default: serde_json::Value,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    /// choice の選択肢。`{ "label": "高画質", "value": "slow" }` または文字列。
    #[serde(default)]
    pub options: Vec<serde_json::Value>,
}

fn default_param_type() -> String {
    "number".into()
}

impl ExporterParam {
    /// 選択肢の (表示名, 引数に入る値)。
    pub fn choices(&self) -> Vec<(String, String)> {
        self.options
            .iter()
            .map(|o| match o {
                serde_json::Value::Object(m) => {
                    let label = m.get("label").map(json_text).unwrap_or_default();
                    let value = m.get("value").map(json_text).unwrap_or_else(|| label.clone());
                    (label, value)
                }
                v => (json_text(v), json_text(v)),
            })
            .collect()
    }

    /// 既定値を引数に入る文字列にする。
    pub fn default_value(&self) -> String {
        match self.kind.as_str() {
            "choice" => {
                let i = self.default.as_u64().unwrap_or(0) as usize;
                self.choices().get(i).map(|c| c.1.clone()).unwrap_or_default()
            }
            "bool" => (self.default.as_bool().unwrap_or(false) as u8).to_string(),
            "args" => json_text(&self.default),
            _ => json_text(&self.default),
        }
    }
}

fn json_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// プラグインが定義する書き出し形式（FFmpeg の引数の組）。
#[derive(Debug, Clone, PartialEq)]
pub struct Exporter {
    /// `パッケージID/ID`
    pub key: String,
    pub plugin: String,
    pub label: String,
    pub extension: String,
    /// 映像の出力引数（`-c:v libx264 -crf {crf}` など）。`{パラメータID}` は値に置き換わる。
    pub video_args: Vec<String>,
    /// 音声の出力引数（空なら音声なし。`audio_only` では必須）。
    pub audio_args: Vec<String>,
    /// アルファ付きでパイプに流す（透過の書き出しができる）。
    pub alpha: bool,
    /// 偶数の解像度が必要（4:2:0 など）。
    pub even: bool,
    /// 音声だけを書き出す（映像はレンダリングしない）。
    pub audio_only: bool,
    /// 1 フレームだけ書き出す（静止画）。範囲の先頭のフレームを使う。
    pub single_frame: bool,
    pub params: Vec<ExporterParam>,
    pub description: String,
}

impl Exporter {
    /// `{id}` を値で置き換えた引数。値の無いものは既定値。空になった引数は取り除く。
    pub fn expand(&self, args: &[String], values: &std::collections::HashMap<String, String>) -> Vec<String> {
        let value = |p: &ExporterParam| values.get(&p.id).cloned().unwrap_or_else(|| p.default_value());
        args.iter()
            .flat_map(|a| {
                // 引数そのものが "{ID}" で、そのパラメータが args 型なら、空白で区切って複数の引数にする
                if let Some(p) = self.params.iter().find(|p| p.kind == "args" && *a == format!("{{{}}}", p.id)) {
                    return value(p).split_whitespace().map(String::from).collect::<Vec<_>>();
                }
                vec![a.clone()]
            })
            .map(|a| {
                let mut out = a.clone();
                for p in &self.params {
                    let v = values.get(&p.id).cloned().unwrap_or_else(|| p.default_value());
                    out = out.replace(&format!("{{{}}}", p.id), &v);
                }
                out
            })
            .filter(|a| !a.is_empty())
            .collect()
    }
}

// ---------- 書き出しの引数の検査 ----------

/// 値を 1 つ取るオプション（ストリーム指定 `:v` などを除いた名前）。エンコードの設定だけを許し、
/// 入出力のファイル・ネットワーク・外部のスクリプトやプラグインを読むものは許さない。
const FFMPEG_OPTS: &[&str] = &[
    "c", "codec", "b", "crf", "cq", "qp", "qp_i", "qp_p", "qp_b", "q", "qscale", "global_quality", "preset", "tune", "profile", "level",
    "pix_fmt", "movflags", "tag", "g", "bf", "keyint_min", "sc_threshold", "maxrate", "minrate", "bufsize", "rc", "quality", "cpu-used",
    "deadline", "row-mt", "tile-columns", "tile-rows", "lossless", "compression_level", "effort", "distance", "loop", "plays", "frames",
    "r", "vf", "af", "filter", "ac", "ar", "sample_fmt", "channel_layout", "vsync", "fps_mode", "aspect", "metadata", "color_primaries",
    "color_trc", "colorspace", "color_range", "f", "threads", "speed", "usage", "aq-mode", "lag-in-frames", "x264-params", "x265-params",
    "svtav1-params", "vtag", "atag", "vbr", "application", "frame_duration", "cutoff", "joint_stereo", "reservoir", "vendor", "alpha_quality",
    "bits_per_mb", "quant_mat", "rc_mode", "bitrate", "multipass", "spatial-aq", "temporal-aq", "b_ref_mode", "lookahead", "refs",
    "allow_sw", "realtime", "prio_speed", "async_depth", "look_ahead", "look_ahead_depth", "extbrc", "low_power", "max_frame_size",
    "qmin", "qmax", "crf_max", "enable-tpl", "lossless_mode", "pred", "prediction", "method", "dither", "duration", "t", "fs",
];
/// 値を取らないオプション。
const FFMPEG_FLAGS: &[&str] = &["an", "vn", "sn", "dn", "shortest"];
/// `-f` で選べる出力の形式（分割・中継・配信など、別のファイルや場所へ書けるものは除く）。
const FFMPEG_FORMATS: &[&str] = &[
    "mp4", "mov", "m4a", "ipod", "matroska", "webm", "gif", "apng", "avif", "image2", "mjpeg", "mp3", "adts", "wav", "flac", "ogg", "opus",
    "webp", "asf", "avi", "mpegts", "ivf", "aiff", "caf", "w64", "null", "h264", "hevc", "rawvideo", "yuv4mpegpipe", "mxf", "ac3", "eac3",
];
/// フィルターグラフで使えるフィルター（ファイルを読む `movie`・コマンドを受ける `sendcmd`・
/// 外部プラグインを読む `frei0r`・`ladspa` などは含めない）。
const FFMPEG_FILTERS: &[&str] = &[
    "scale", "crop", "pad", "fps", "split", "asplit", "palettegen", "paletteuse", "format", "aformat", "setsar", "setdar", "transpose",
    "hflip", "vflip", "rotate", "eq", "colorspace", "zscale", "tonemap", "unsharp", "boxblur", "gblur", "hqdn3d", "deband", "null",
    "anull", "aresample", "volume", "loudnorm", "atempo", "afade", "fade", "trim", "atrim", "setpts", "asetpts", "tpad", "apad",
    "colorchannelmixer", "curves", "hue", "lutyuv", "lutrgb", "lut", "chromakey", "colorkey", "overlay", "premultiply", "unpremultiply",
    "alphaextract", "alphamerge", "geq", "fieldorder", "yadif", "bwdif", "deinterlace", "minterpolate", "framerate", "tblend", "blend",
    "colorlevels", "colorbalance", "vibrance", "noise", "dctdnoiz", "nlmeans", "smartblur", "sharpen", "convolution", "drawbox", "drawgrid",
    "showinfo", "select", "aselect", "reverse", "areverse", "loop", "aloop", "highpass", "lowpass", "equalizer", "compand", "acompressor",
    "alimiter", "dynaudnorm", "pan", "amix", "amerge", "channelsplit", "channelmap", "silenceremove", "aecho", "chorus", "flanger",
    "tremolo", "vibrato", "stereotools", "extrastereo", "bass", "treble", "earwax", "crystalizer", "libplacebo", "scale_vt", "tonemap_vt",
    "hwupload", "hwdownload", "format_cuda", "scale_cuda", "scale_qsv", "vpp_qsv", "settb", "asettb", "colormatrix", "lenscorrection",
    "vignette", "edgedetect", "negate", "swapuv", "shuffleplanes", "extractplanes", "mergeplanes", "histeq", "normalize", "dejudder",
];

/// 値に、ファイル・ネットワーク・外部のファイルを指すものが無いか。
fn check_ffmpeg_value(name: &str, v: &str) -> Result<(), String> {
    let lower = v.to_ascii_lowercase();
    let bad = lower.contains("://")
        || v.starts_with('/')
        || v.starts_with('~')
        || v.starts_with('@')
        || v.contains('\\')
        || v.contains("..")
        || ["file:", "pipe:", "concat:", "subfile:", "data:", "fd:", "tcp:", "udp:", "unix:", "cache:", "crypto:"].iter().any(|p| lower.contains(p))
        || lower.as_bytes().get(1) == Some(&b':') && lower.as_bytes().first().is_some_and(|c| c.is_ascii_alphabetic()) && lower.as_bytes().get(2).is_some_and(|c| *c == b'/' || *c == b'\\');
    if bad {
        return Err(format!("-{name} {v}: ファイルや通信先を指す値は使えません"));
    }
    if name.ends_with("-params") && ["stats", "csv", "dump", "qpfile", "zones-file", "input"].iter().any(|k| lower.contains(k)) {
        return Err(format!("-{name} {v}: ファイルを読み書きする設定は使えません"));
    }
    match name {
        "f" if !FFMPEG_FORMATS.contains(&lower.as_str()) => Err(format!("-f {v}: この出力の形式は使えません")),
        "vf" | "af" | "filter" => {
            // フィルターの名前（`[a]` などのラベルと `=` 以降の引数を除いたもの）を調べる
            for part in split_filtergraph(v) {
                let mut s = part.trim();
                while let Some(rest) = s.strip_prefix('[') {
                    s = rest.split_once(']').map(|(_, r)| r.trim_start()).unwrap_or("");
                }
                let fname = s.split(['=', '[']).next().unwrap_or("").trim();
                if fname.is_empty() {
                    continue;
                }
                if !FFMPEG_FILTERS.contains(&fname) {
                    return Err(format!("フィルター「{fname}」は使えません（ファイルや外部のプログラムを読むおそれがあるため、決まったものだけ使えます）"));
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// フィルターグラフをフィルターごとに分ける（`'...'` の中と括弧の中の `,` `;` では分けない）。
fn split_filtergraph(v: &str) -> Vec<&str> {
    let (mut out, mut start, mut quoted, mut depth, mut escaped) = (Vec::new(), 0, false, 0i32, false);
    for (i, c) in v.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '\'' => quoted = !quoted,
            '(' if !quoted => depth += 1,
            ')' if !quoted => depth -= 1,
            ',' | ';' if !quoted && depth <= 0 => {
                out.push(&v[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&v[start..]);
    out
}

/// プラグインの書き出し形式の FFmpeg の引数（展開したあと）が安全か調べる。
///
/// 書き出し先・入力はホストが決め、引数はエンコードの設定だけにする。決まったオプションだけを許し、
/// オプションの値でない引数（FFmpeg は出力ファイルとして扱う）、ファイル・通信先を指す値、
/// ファイルや外部のプログラムを読むフィルター、分割・中継のための出力形式は拒む。
pub fn check_ffmpeg_args(args: &[String]) -> Result<(), String> {
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let Some(opt) = a.strip_prefix('-').filter(|o| !o.is_empty()) else {
            return Err(format!("「{a}」: オプションの値ではない引数（出力ファイルとして扱われる）は使えません"));
        };
        let (name, spec) = opt.split_once(':').unwrap_or((opt, ""));
        if !spec.chars().all(|c| c.is_ascii_alphanumeric() || c == ':') {
            return Err(format!("「{a}」: ストリームの指定が不正です"));
        }
        if FFMPEG_FLAGS.contains(&name) {
            i += 1;
            continue;
        }
        if !FFMPEG_OPTS.contains(&name) {
            return Err(format!("「{a}」: このオプションは書き出し形式では使えません"));
        }
        let Some(v) = args.get(i + 1) else { return Err(format!("「{a}」: 値がありません")) };
        check_ffmpeg_value(name, v)?;
        i += 2;
    }
    Ok(())
}

fn exporters_list() -> &'static Mutex<Vec<Exporter>> {
    static E: OnceLock<Mutex<Vec<Exporter>>> = OnceLock::new();
    E.get_or_init(Mutex::default)
}

pub fn register_exporter(e: Exporter) {
    let mut list = exporters_list().lock().unwrap();
    if let Some(old) = list.iter_mut().find(|o| o.key == e.key) {
        *old = e;
    } else {
        list.push(e);
    }
}

/// 読み込んだプラグインが定義した書き出し形式。
pub fn exporters() -> Vec<Exporter> {
    exporters_list().lock().unwrap().clone()
}

pub fn add_expression_library(name: &str, source: &str) {
    let mut libs = libraries().lock().unwrap();
    let mut list: Vec<(String, String)> = libs.as_ref().clone();
    list.retain(|(n, _)| n != name);
    list.push((name.to_string(), source.to_string()));
    *libs = Arc::new(list);
    LIB_GENERATION.fetch_add(1, Ordering::Relaxed);
    bump();
}

/// （世代, ライブラリ一覧）。エクスプレッションエンジンがスレッドごとに読み込み直すのに使う。
pub fn expression_libraries() -> (u64, Arc<Vec<(String, String)>>) {
    let libs = libraries().lock().unwrap().clone();
    (LIB_GENERATION.load(Ordering::Relaxed), libs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NoFootage, RenderOptions, Renderer};

    #[test]
    fn output_filter_applies_per_stage_and_changes_fingerprint() {
        let project = znd_core::demo::demo_project();
        let comp = project.compositions[0].id.clone();
        let t = Time::ZERO;
        let r = Renderer::new(&project, &NoFootage);
        let opts = RenderOptions { downsample: 8, ..RenderOptions::PREVIEW };
        let before = r.render_frame(&comp, t, opts).unwrap();
        let fp = crate::fingerprint::Fingerprinter::new(&project);
        let k0 = fp.frame(&project, &comp, t, opts);
        let mut params = serde_json::Map::new();
        params.insert("saturation".into(), serde_json::json!(-100));
        register_filter(OutputFilter {
            key: "test/bw".into(),
            plugin: "test".into(),
            label: "白黒".into(),
            stage: Stage::Preview,
            enabled: true,
            effects: vec![FilterEffect { kind: "hue_saturation".into(), params }],
        });
        let after = r.render_frame(&comp, t, opts).unwrap();
        assert_ne!(fp.frame(&project, &comp, t, opts), k0, "キャッシュの指紋が変わること");
        let gray = after.data.chunks_exact(4).all(|p| (p[0] - p[1]).abs() < 1e-3 && (p[1] - p[2]).abs() < 1e-3);
        assert!(gray && after.data != before.data, "プレビューは白黒になる");
        // 書き出しには掛からない
        let export = r.render_frame(&comp, t, RenderOptions { downsample: 8, ..RenderOptions::EXPORT }).unwrap();
        assert!(export.data.chunks_exact(4).any(|p| (p[0] - p[2]).abs() > 1e-2));
        set_filter_enabled("test/bw", false);
    }

    #[test]
    fn ffmpeg_args_allowlist() {
        let ok = |a: &[&str]| check_ffmpeg_args(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert!(ok(&["-c:v", "libx264", "-preset", "medium", "-crf", "18", "-pix_fmt", "yuv420p", "-movflags", "+faststart"]).is_ok());
        assert!(ok(&["-vf", "fps=15,split[a][b];[a]palettegen=max_colors=64[p];[b][p]paletteuse=dither=bayer", "-loop", "0"]).is_ok());
        assert!(ok(&["-vf", "crop=w='min(100,iw-0)':h='min(100,ih-0)':x='min(0,iw-1)':y='min(0,ih-1)'", "-frames:v", "1"]).is_ok());
        assert!(ok(&["-f", "apng", "-plays", "0"]).is_ok());
        assert!(ok(&["-an"]).is_ok());
        // 出力ファイルの追加・入力・別の場所への書き込み・ファイルを読むフィルター・外部プラグイン
        assert!(ok(&["-c:v", "libx264", "/tmp/steal.mp4"]).is_err());
        assert!(ok(&["-i", "/etc/passwd"]).is_err());
        assert!(ok(&["-y"]).is_err());
        assert!(ok(&["-f", "tee"]).is_err());
        assert!(ok(&["-f", "segment"]).is_err());
        assert!(ok(&["-vf", "movie=/etc/passwd"]).is_err());
        assert!(ok(&["-vf", "scale=100:100,frei0r=filter_name=x"]).is_err());
        assert!(ok(&["-af", "ladspa=file=x"]).is_err());
        assert!(ok(&["-metadata", "title=http://example.com/x"]).is_err());
        assert!(ok(&["-x265-params", "stats=x.log"]).is_err());
        assert!(ok(&["-filter_complex_script", "x"]).is_err());
        assert!(ok(&["-c:v"]).is_err());
        assert!(ok(&["-vf", "scale=1:1", "-c:v", "C:/x"]).is_err());
    }
}
