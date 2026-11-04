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
                audio: &no_audio,
                eval_at: &eval_at,
                masks: &[],
                seed,
                offset: &offset,
                layer_time: t.seconds(),
                layer_duration: comp.duration.seconds(),
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
}
