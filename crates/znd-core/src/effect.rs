//! エフェクトのデータモデルと定義表。処理本体は `znd-render::effects`。
//!
//! パラメータは名前付きの汎用値なので、プリセット（JSON）としてそのまま保存・共有でき、
//! キーフレームとエクスプレッションも一緒に運ばれる。

use crate::model::LayerId;
use crate::property::Property;
use crate::time::Time;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", content = "v", rename_all = "snake_case")]
pub enum ParamValue {
    Scalar(Property<f64>),
    /// レイヤー座標（px）。
    Point(Property<[f64; 2]>),
    /// sRGB RGBA。
    Color(Property<[f64; 4]>),
    Bool(bool),
    Choice(u32),
    Layer(Option<LayerId>),
    Text(String),
    /// 点列（トーンカーブの制御点、メッシュの変位など）。
    Curve(Vec<[f64; 2]>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectInstance {
    pub kind: String,
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub params: BTreeMap<String, ParamValue>,
}

fn yes() -> bool {
    true
}

/// 時刻 t で評価済みのパラメータ値。
#[derive(Debug, Clone, PartialEq)]
pub enum EvalValue {
    F(f64),
    P([f64; 2]),
    C([f64; 4]),
    B(bool),
    I(u32),
    L(Option<LayerId>),
    S(String),
    Curve(Vec<[f64; 2]>),
}

#[derive(Debug, Clone, Default)]
pub struct EvalParams(pub BTreeMap<String, EvalValue>);

impl EvalParams {
    pub fn f(&self, name: &str) -> f64 {
        match self.0.get(name) {
            Some(EvalValue::F(v)) => *v,
            Some(EvalValue::B(b)) => *b as u8 as f64,
            Some(EvalValue::I(i)) => *i as f64,
            _ => 0.0,
        }
    }
    pub fn p(&self, name: &str) -> [f64; 2] {
        match self.0.get(name) {
            Some(EvalValue::P(v)) => *v,
            _ => [0.0; 2],
        }
    }
    pub fn c(&self, name: &str) -> [f64; 4] {
        match self.0.get(name) {
            Some(EvalValue::C(v)) => *v,
            _ => [0.0, 0.0, 0.0, 1.0],
        }
    }
    pub fn b(&self, name: &str) -> bool {
        matches!(self.0.get(name), Some(EvalValue::B(true)))
    }
    pub fn i(&self, name: &str) -> u32 {
        match self.0.get(name) {
            Some(EvalValue::I(v)) => *v,
            _ => 0,
        }
    }
    pub fn layer(&self, name: &str) -> Option<&LayerId> {
        match self.0.get(name) {
            Some(EvalValue::L(v)) => v.as_ref(),
            _ => None,
        }
    }
    pub fn s(&self, name: &str) -> &str {
        match self.0.get(name) {
            Some(EvalValue::S(v)) => v,
            _ => "",
        }
    }
    pub fn curve(&self, name: &str) -> &[[f64; 2]] {
        match self.0.get(name) {
            Some(EvalValue::Curve(v)) => v,
            _ => &[],
        }
    }
}

impl EffectInstance {
    /// 既定値でエフェクトを作る。ポイントの既定値はレイヤーサイズに対する割合から px に直す。
    pub fn new(kind: &str, layer_w: f64, layer_h: f64) -> Option<Self> {
        let def = effect_def(kind)?;
        let params = def
            .params
            .iter()
            .map(|p| {
                let v = match &p.kind {
                    Pd::F(d, _, _) => ParamValue::Scalar(Property::constant(*d)),
                    Pd::P(x, y) => ParamValue::Point(Property::constant([x * layer_w, y * layer_h])),
                    Pd::C(c) => ParamValue::Color(Property::constant(*c)),
                    Pd::B(b) => ParamValue::Bool(*b),
                    Pd::I(d, _) => ParamValue::Choice(*d),
                    Pd::L => ParamValue::Layer(None),
                    Pd::S => ParamValue::Text(String::new()),
                    Pd::Curve(pts) => ParamValue::Curve(pts.to_vec()),
                };
                (p.name.to_string(), v)
            })
            .collect();
        Some(EffectInstance { kind: kind.to_string(), name: def.label.to_string(), enabled: true, params })
    }

    pub fn set_f(mut self, name: &str, v: f64) -> Self {
        self.params.insert(name.into(), ParamValue::Scalar(Property::constant(v)));
        self
    }
    pub fn set_p(mut self, name: &str, v: [f64; 2]) -> Self {
        self.params.insert(name.into(), ParamValue::Point(Property::constant(v)));
        self
    }
    pub fn set_c(mut self, name: &str, v: [f64; 4]) -> Self {
        self.params.insert(name.into(), ParamValue::Color(Property::constant(v)));
        self
    }
    pub fn set_b(mut self, name: &str, v: bool) -> Self {
        self.params.insert(name.into(), ParamValue::Bool(v));
        self
    }
    pub fn set_i(mut self, name: &str, v: u32) -> Self {
        self.params.insert(name.into(), ParamValue::Choice(v));
        self
    }
    pub fn set_layer(mut self, name: &str, v: &str) -> Self {
        self.params.insert(name.into(), ParamValue::Layer(Some(LayerId::new(v))));
        self
    }
    pub fn set_s(mut self, name: &str, v: &str) -> Self {
        self.params.insert(name.into(), ParamValue::Text(v.into()));
        self
    }
    pub fn set_curve(mut self, name: &str, v: Vec<[f64; 2]>) -> Self {
        self.params.insert(name.into(), ParamValue::Curve(v));
        self
    }
    pub fn set_prop(mut self, name: &str, p: Property<f64>) -> Self {
        self.params.insert(name.into(), ParamValue::Scalar(p));
        self
    }

    pub fn eval(&self, t: Time) -> EvalParams {
        EvalParams(
            self.params
                .iter()
                .map(|(k, v)| {
                    let e = match v {
                        ParamValue::Scalar(p) => EvalValue::F(p.eval(t)),
                        ParamValue::Point(p) => EvalValue::P(p.eval(t)),
                        ParamValue::Color(p) => EvalValue::C(p.eval(t)),
                        ParamValue::Bool(b) => EvalValue::B(*b),
                        ParamValue::Choice(i) => EvalValue::I(*i),
                        ParamValue::Layer(l) => EvalValue::L(l.clone()),
                        ParamValue::Text(s) => EvalValue::S(s.clone()),
                        ParamValue::Curve(c) => EvalValue::Curve(c.clone()),
                    };
                    (k.clone(), e)
                })
                .collect(),
        )
    }

    /// アニメーションまたはエクスプレッションを持つパラメータがあるか。
    pub fn is_animated(&self) -> bool {
        self.params.values().any(|v| match v {
            ParamValue::Scalar(p) => p.is_animated() || p.expression.is_some(),
            ParamValue::Point(p) => p.is_animated() || p.expression.is_some(),
            ParamValue::Color(p) => p.is_animated() || p.expression.is_some(),
            _ => false,
        })
    }
}

/// パラメータ定義。
pub enum Pd {
    /// (既定値, 最小, 最大)
    F(f64, f64, f64),
    /// 既定位置（レイヤー幅・高さに対する割合）
    P(f64, f64),
    C([f64; 4]),
    B(bool),
    /// (既定, 選択肢のラベル)
    I(u32, &'static [&'static str]),
    L,
    S,
    Curve(&'static [[f64; 2]]),
}

pub struct ParamDef {
    pub name: &'static str,
    pub label: &'static str,
    pub kind: Pd,
}

pub struct EffectDef {
    pub kind: &'static str,
    pub label: &'static str,
    pub category: &'static str,
    /// 時刻そのものに依存する（パラメータが静止していても毎フレーム結果が変わる）。
    pub time_dependent: bool,
    pub params: &'static [ParamDef],
}

const fn p(name: &'static str, label: &'static str, kind: Pd) -> ParamDef {
    ParamDef { name, label, kind }
}

const WHITE: [f64; 4] = [1.0, 1.0, 1.0, 1.0];
const BLACK: [f64; 4] = [0.0, 0.0, 0.0, 1.0];
const BIG: f64 = 1e6;

macro_rules! fx {
    ($kind:literal, $label:literal, $cat:literal, $td:literal, [$($p:expr),* $(,)?]) => {
        EffectDef { kind: $kind, label: $label, category: $cat, time_dependent: $td, params: &[$($p),*] }
    };
}

pub static EFFECTS: &[EffectDef] = &[
    // ---- ブラー ----
    fx!(
        "gaussian_blur",
        "ブラー（ガウス）",
        "ブラー",
        false,
        [
            p("radius", "ブラー", Pd::F(10.0, 0.0, 2000.0)),
            p("dimensions", "方向", Pd::I(0, &["水平および垂直", "水平", "垂直"])),
            p("repeat_edge", "エッジピクセルを繰り返す", Pd::B(false)),
        ]
    ),
    fx!(
        "directional_blur",
        "ブラー（方向）",
        "ブラー",
        false,
        [p("angle", "方向", Pd::F(0.0, -BIG, BIG)), p("length", "ブラーの長さ", Pd::F(20.0, 0.0, 2000.0)),]
    ),
    fx!(
        "radial_blur",
        "ブラー（放射状）",
        "ブラー",
        false,
        [p("center", "中心", Pd::P(0.5, 0.5)), p("amount", "量（度）", Pd::F(10.0, 0.0, 360.0)),]
    ),
    fx!(
        "zoom_blur",
        "ブラー（ズーム）",
        "ブラー",
        false,
        [p("center", "中心", Pd::P(0.5, 0.5)), p("amount", "量（%）", Pd::F(20.0, 0.0, 200.0)),]
    ),
    fx!(
        "lens_blur",
        "ブラー（カメラレンズ）",
        "ブラー",
        false,
        [
            p("radius", "ブラーの半径", Pd::F(10.0, 0.0, 500.0)),
            p("sides", "絞りの羽根枚数", Pd::F(6.0, 3.0, 16.0)),
            p("rotation", "絞りの回転", Pd::F(0.0, -BIG, BIG)),
            p("highlight_gain", "ハイライトのゲイン", Pd::F(0.0, 0.0, 10.0)),
            p("highlight_threshold", "ハイライトのしきい値", Pd::F(0.9, 0.0, 10.0)),
        ]
    ),
    fx!(
        "compound_blur",
        "ブラー（可変）",
        "ブラー",
        false,
        [p("map", "ブラーレイヤー", Pd::L), p("max_radius", "最大ブラー", Pd::F(20.0, 0.0, 500.0)), p("invert", "反転", Pd::B(false)),]
    ),
    // ---- 光 ----
    fx!(
        "glow",
        "グロー",
        "光",
        false,
        [
            p("threshold", "しきい値", Pd::F(0.6, 0.0, 10.0)),
            p("radius", "半径", Pd::F(30.0, 0.0, 1000.0)),
            p("intensity", "強度", Pd::F(1.0, 0.0, 20.0)),
            p("layers", "段数", Pd::F(4.0, 1.0, 8.0)),
            p("color", "色合い", Pd::C(WHITE)),
        ]
    ),
    fx!(
        "bloom",
        "ブルーム",
        "光",
        false,
        [
            p("threshold", "しきい値", Pd::F(0.8, 0.0, 10.0)),
            p("radius", "半径", Pd::F(40.0, 0.0, 1000.0)),
            p("intensity", "強度", Pd::F(0.6, 0.0, 20.0)),
        ]
    ),
    fx!(
        "lens_flare",
        "レンズフレア",
        "光",
        false,
        [
            p("center", "光源の位置", Pd::P(0.3, 0.3)),
            p("brightness", "明るさ（%）", Pd::F(100.0, 0.0, 400.0)),
            p("color", "色", Pd::C([1.0, 0.9, 0.7, 1.0])),
        ]
    ),
    fx!(
        "light_rays",
        "光線",
        "光",
        false,
        [
            p("center", "光源の位置", Pd::P(0.5, 0.5)),
            p("length", "光線の長さ（%）", Pd::F(40.0, 0.0, 100.0)),
            p("threshold", "しきい値", Pd::F(0.5, 0.0, 10.0)),
            p("intensity", "強度", Pd::F(1.0, 0.0, 20.0)),
            p("color", "色", Pd::C(WHITE)),
        ]
    ),
    fx!(
        "drop_shadow",
        "ドロップシャドウ",
        "光",
        false,
        [
            p("color", "色", Pd::C(BLACK)),
            p("opacity", "不透明度", Pd::F(50.0, 0.0, 100.0)),
            p("angle", "方向", Pd::F(135.0, -BIG, BIG)),
            p("distance", "距離", Pd::F(10.0, 0.0, 2000.0)),
            p("softness", "柔らかさ", Pd::F(8.0, 0.0, 500.0)),
        ]
    ),
    // ---- 歪み ----
    fx!(
        "displacement_map",
        "ディスプレイスメントマップ",
        "ディストーション",
        false,
        [
            p("map", "マップレイヤー", Pd::L),
            p("max_h", "最大水平置き換え", Pd::F(20.0, -4000.0, 4000.0)),
            p("max_v", "最大垂直置き換え", Pd::F(20.0, -4000.0, 4000.0)),
            p("channel_h", "水平に使用", Pd::I(0, &["赤", "緑", "青", "アルファ", "輝度"])),
            p("channel_v", "垂直に使用", Pd::I(1, &["赤", "緑", "青", "アルファ", "輝度"])),
        ]
    ),
    fx!(
        "turbulent_displace",
        "タービュレントディスプレイス",
        "ディストーション",
        false,
        [
            p("amount", "量", Pd::F(30.0, 0.0, 1000.0)),
            p("size", "サイズ", Pd::F(100.0, 2.0, 2000.0)),
            p("evolution", "展開", Pd::F(0.0, -BIG, BIG)),
            p("complexity", "複雑度", Pd::F(2.0, 1.0, 8.0)),
        ]
    ),
    fx!(
        "wave_warp",
        "波形ワープ",
        "ディストーション",
        true,
        [
            p("height", "波形の高さ", Pd::F(10.0, -1000.0, 1000.0)),
            p("width", "波形の幅", Pd::F(60.0, 1.0, 4000.0)),
            p("direction", "方向", Pd::F(90.0, -BIG, BIG)),
            p("speed", "波形の速度", Pd::F(1.0, -100.0, 100.0)),
            p("phase", "フェーズ", Pd::F(0.0, -BIG, BIG)),
            p("wave_type", "波形の種類", Pd::I(0, &["サイン", "矩形", "三角", "ノコギリ"])),
        ]
    ),
    fx!(
        "polar_coordinates",
        "極座標",
        "ディストーション",
        false,
        [
            p("interpolation", "補間（%）", Pd::F(100.0, 0.0, 100.0)),
            p("mode", "変換の種類", Pd::I(0, &["長方形から極線へ", "極線から長方形へ"])),
        ]
    ),
    fx!(
        "mesh_warp",
        "メッシュワープ",
        "ディストーション",
        false,
        [
            p("amount", "量（%）", Pd::F(100.0, 0.0, 100.0)),
            // 4x4 の格子点の変位（px）。行優先。
            p("offsets", "格子点の変位", Pd::Curve(&[[0.0, 0.0]; 16])),
        ]
    ),
    fx!(
        "liquify",
        "液化",
        "ディストーション",
        false,
        [
            p("amount", "量（%）", Pd::F(100.0, 0.0, 100.0)),
            p("radius", "ブラシサイズ", Pd::F(80.0, 1.0, 2000.0)),
            // [始点, 終点] の対の列。各対が 1 回の「押し」。
            p("strokes", "ストローク", Pd::Curve(&[])),
        ]
    ),
    fx!(
        "mirror",
        "ミラー",
        "ディストーション",
        false,
        [p("center", "反射の中心", Pd::P(0.5, 0.5)), p("angle", "反射角度", Pd::F(0.0, -BIG, BIG)),]
    ),
    fx!(
        "motion_tile",
        "モーションタイル",
        "ディストーション",
        false,
        [
            p("center", "タイルの中心", Pd::P(0.5, 0.5)),
            p("tile_width", "タイルの幅（%）", Pd::F(100.0, 1.0, 100.0)),
            p("tile_height", "タイルの高さ（%）", Pd::F(100.0, 1.0, 100.0)),
            p("mirror_edges", "ミラーエッジ", Pd::B(false)),
            p("phase", "フェーズ", Pd::F(0.0, -BIG, BIG)),
        ]
    ),
    // ---- 生成 ----
    fx!(
        "fractal_noise",
        "フラクタルノイズ",
        "描画",
        false,
        [
            p("fractal_type", "フラクタルの種類", Pd::I(0, &["基本", "乱気流"])),
            p("contrast", "コントラスト", Pd::F(100.0, 0.0, 1000.0)),
            p("brightness", "明るさ", Pd::F(0.0, -200.0, 200.0)),
            p("scale", "スケール", Pd::F(100.0, 1.0, 10000.0)),
            p("complexity", "複雑度", Pd::F(6.0, 1.0, 10.0)),
            p("evolution", "展開", Pd::F(0.0, -BIG, BIG)),
            p("offset", "乱気流のオフセット", Pd::P(0.5, 0.5)),
            p("loop_evolution", "サイクル展開", Pd::B(false)),
            p("cycle", "サイクル（周回）", Pd::F(1.0, 1.0, 30.0)),
            p("opacity", "不透明度", Pd::F(100.0, 0.0, 100.0)),
            p("blend", "描画モード", Pd::I(0, &["なし（置換）", "加算", "乗算", "スクリーン"])),
        ]
    ),
    fx!(
        "gradient_ramp",
        "グラデーション",
        "描画",
        false,
        [
            p("start", "開始点", Pd::P(0.5, 0.0)),
            p("start_color", "開始色", Pd::C(BLACK)),
            p("end", "終了点", Pd::P(0.5, 1.0)),
            p("end_color", "終了色", Pd::C(WHITE)),
            p("shape", "形状", Pd::I(0, &["直線", "放射状"])),
            p("opacity", "元の画像とブレンド（%）", Pd::F(0.0, 0.0, 100.0)),
        ]
    ),
    fx!(
        "cell_pattern",
        "セルパターン",
        "描画",
        false,
        [
            p("mode", "パターン", Pd::I(0, &["泡", "プレート", "クリスタル"])),
            p("size", "サイズ", Pd::F(60.0, 2.0, 2000.0)),
            p("contrast", "コントラスト", Pd::F(100.0, 0.0, 600.0)),
            p("evolution", "展開", Pd::F(0.0, -BIG, BIG)),
        ]
    ),
    fx!(
        "checkerboard",
        "チェッカーボード",
        "描画",
        false,
        [
            p("anchor", "アンカー", Pd::P(0.5, 0.5)),
            p("width", "幅", Pd::F(64.0, 1.0, 4000.0)),
            p("height", "高さ", Pd::F(64.0, 1.0, 4000.0)),
            p("color", "色", Pd::C(WHITE)),
            p("opacity", "不透明度", Pd::F(100.0, 0.0, 100.0)),
        ]
    ),
    fx!(
        "audio_spectrum",
        "オーディオスペクトラム",
        "描画",
        true,
        [
            p("audio_layer", "オーディオレイヤー", Pd::L),
            p("start", "開始ポイント", Pd::P(0.1, 0.5)),
            p("end", "終了ポイント", Pd::P(0.9, 0.5)),
            p("bands", "周波数バンド", Pd::F(64.0, 1.0, 1024.0)),
            p("max_height", "最大高さ", Pd::F(300.0, 0.0, 4000.0)),
            p("start_freq", "開始周波数", Pd::F(20.0, 1.0, 24000.0)),
            p("end_freq", "終了周波数", Pd::F(8000.0, 1.0, 24000.0)),
            p("thickness", "太さ", Pd::F(3.0, 0.5, 200.0)),
            p("color", "色", Pd::C(WHITE)),
        ]
    ),
    fx!(
        "audio_waveform",
        "オーディオウェーブフォーム",
        "描画",
        true,
        [
            p("audio_layer", "オーディオレイヤー", Pd::L),
            p("start", "開始ポイント", Pd::P(0.1, 0.5)),
            p("end", "終了ポイント", Pd::P(0.9, 0.5)),
            p("samples", "表示するサンプル", Pd::F(256.0, 2.0, 4096.0)),
            p("max_height", "最大高さ", Pd::F(200.0, 0.0, 4000.0)),
            p("thickness", "太さ", Pd::F(2.0, 0.5, 200.0)),
            p("color", "色", Pd::C(WHITE)),
        ]
    ),
    fx!(
        "stroke",
        "線",
        "描画",
        false,
        [
            p("mask", "マスク番号", Pd::F(1.0, 1.0, 64.0)),
            p("color", "色", Pd::C(WHITE)),
            p("width", "ブラシのサイズ", Pd::F(4.0, 0.1, 500.0)),
            p("start", "開始（%）", Pd::F(0.0, 0.0, 100.0)),
            p("end", "終了（%）", Pd::F(100.0, 0.0, 100.0)),
            p("on_transparent", "透明上に描画", Pd::B(false)),
        ]
    ),
    fx!(
        "scribble",
        "落書き",
        "描画",
        true,
        [
            p("mask", "マスク番号", Pd::F(1.0, 1.0, 64.0)),
            p("color", "色", Pd::C(WHITE)),
            p("width", "線幅", Pd::F(3.0, 0.1, 200.0)),
            p("angle", "角度", Pd::F(0.0, -BIG, BIG)),
            p("spacing", "間隔", Pd::F(8.0, 1.0, 500.0)),
            p("curviness", "うねり", Pd::F(5.0, 0.0, 200.0)),
            p("wiggles_per_second", "ウィグル／秒", Pd::F(0.0, 0.0, 60.0)),
            p("seed", "ランダムシード", Pd::F(0.0, 0.0, 100000.0)),
        ]
    ),
    // ---- 時間 ----
    fx!(
        "echo",
        "エコー",
        "時間",
        true,
        [
            p("echo_time", "エコー時間（秒）", Pd::F(-0.033, -30.0, 30.0)),
            p("count", "エコーの数", Pd::F(4.0, 0.0, 64.0)),
            p("starting_intensity", "開始強度", Pd::F(1.0, 0.0, 1.0)),
            p("decay", "減衰", Pd::F(0.7, 0.0, 1.0)),
            p("operator", "エコー演算子", Pd::I(0, &["加算", "最大", "スクリーン", "後ろに合成", "前に合成"])),
        ]
    ),
    fx!(
        "afterimage",
        "残像",
        "時間",
        true,
        [p("frames", "残像の数（フレーム）", Pd::F(6.0, 1.0, 64.0)), p("decay", "減衰", Pd::F(0.8, 0.0, 1.0)),]
    ),
    fx!(
        "pixel_motion_blur",
        "ピクセルモーションブラー",
        "時間",
        true,
        [p("shutter_angle", "シャッター角度", Pd::F(180.0, 0.0, 720.0)), p("samples", "シャッターサンプル", Pd::F(8.0, 2.0, 32.0)),]
    ),
    fx!(
        "time_displacement",
        "時間置き換え",
        "時間",
        true,
        [
            p("map", "時間置き換えレイヤー", Pd::L),
            p("max_displacement", "最大時間置き換え（秒）", Pd::F(0.5, -30.0, 30.0)),
            p("steps", "時間解像度（段階）", Pd::F(12.0, 2.0, 60.0)),
        ]
    ),
    // ---- スタイライズ ----
    fx!(
        "glitch",
        "グリッチ",
        "スタイライズ",
        true,
        [
            p("amount", "量（%）", Pd::F(50.0, 0.0, 100.0)),
            p("block_size", "ブロックの高さ", Pd::F(40.0, 2.0, 2000.0)),
            p("rgb_shift", "RGB ずれ", Pd::F(8.0, 0.0, 500.0)),
            p("speed", "速度（回／秒）", Pd::F(10.0, 0.0, 120.0)),
            p("seed", "ランダムシード", Pd::F(0.0, 0.0, 100000.0)),
        ]
    ),
    fx!(
        "rgb_split",
        "色収差（RGB 分離）",
        "スタイライズ",
        false,
        [p("amount", "量", Pd::F(8.0, 0.0, 1000.0)), p("angle", "角度", Pd::F(0.0, -BIG, BIG)), p("radial", "放射状", Pd::B(false)),]
    ),
    fx!("posterize", "ポスタリゼーション", "スタイライズ", false, [p("levels", "レベル", Pd::F(6.0, 2.0, 255.0))]),
    fx!(
        "halftone",
        "ハーフトーン",
        "スタイライズ",
        false,
        [
            p("dot_size", "ドットのサイズ", Pd::F(8.0, 2.0, 500.0)),
            p("angle", "角度", Pd::F(45.0, -BIG, BIG)),
            p("color", "カラー（CMYK 風）", Pd::B(false)),
        ]
    ),
    fx!(
        "mosaic",
        "モザイク",
        "スタイライズ",
        false,
        [p("h_blocks", "水平ブロック", Pd::F(40.0, 1.0, 4000.0)), p("v_blocks", "垂直ブロック", Pd::F(22.0, 1.0, 4000.0)),]
    ),
    fx!(
        "find_edges",
        "輪郭検出",
        "スタイライズ",
        false,
        [p("invert", "反転", Pd::B(false)), p("blend", "元の画像とブレンド（%）", Pd::F(0.0, 0.0, 100.0)),]
    ),
    fx!(
        "noise",
        "ノイズ",
        "スタイライズ",
        true,
        [
            p("amount", "ノイズ量（%）", Pd::F(10.0, 0.0, 100.0)),
            p("color_noise", "カラーノイズ", Pd::B(true)),
            p("animated", "フレームごとに変化", Pd::B(true)),
        ]
    ),
    fx!(
        "film_grain",
        "フィルムグレイン",
        "スタイライズ",
        true,
        [p("amount", "強度（%）", Pd::F(15.0, 0.0, 100.0)), p("size", "粒子サイズ", Pd::F(1.5, 0.5, 20.0)),]
    ),
    fx!(
        "scanlines",
        "スキャンライン",
        "スタイライズ",
        true,
        [
            p("spacing", "間隔", Pd::F(4.0, 2.0, 500.0)),
            p("darkness", "暗さ（%）", Pd::F(50.0, 0.0, 100.0)),
            p("speed", "流れる速度（px／秒）", Pd::F(0.0, -2000.0, 2000.0)),
        ]
    ),
    // ---- カラー ----
    fx!(
        "levels",
        "レベル補正",
        "カラー補正",
        false,
        [
            p("in_black", "黒入力レベル", Pd::F(0.0, -1.0, 2.0)),
            p("in_white", "白入力レベル", Pd::F(1.0, -1.0, 4.0)),
            p("gamma", "ガンマ", Pd::F(1.0, 0.01, 10.0)),
            p("out_black", "黒出力レベル", Pd::F(0.0, -1.0, 2.0)),
            p("out_white", "白出力レベル", Pd::F(1.0, -1.0, 4.0)),
        ]
    ),
    fx!("curves", "トーンカーブ", "カラー補正", false, [p("curve", "カーブ", Pd::Curve(&[[0.0, 0.0], [1.0, 1.0]]))]),
    fx!(
        "hue_saturation",
        "色相／彩度",
        "カラー補正",
        false,
        [
            p("hue", "色相", Pd::F(0.0, -BIG, BIG)),
            p("saturation", "彩度", Pd::F(0.0, -100.0, 100.0)),
            p("lightness", "明度", Pd::F(0.0, -100.0, 100.0)),
        ]
    ),
    fx!(
        "color_balance",
        "カラーバランス",
        "カラー補正",
        false,
        [
            p("shadow_r", "シャドウ 赤", Pd::F(0.0, -100.0, 100.0)),
            p("shadow_g", "シャドウ 緑", Pd::F(0.0, -100.0, 100.0)),
            p("shadow_b", "シャドウ 青", Pd::F(0.0, -100.0, 100.0)),
            p("mid_r", "ミッドトーン 赤", Pd::F(0.0, -100.0, 100.0)),
            p("mid_g", "ミッドトーン 緑", Pd::F(0.0, -100.0, 100.0)),
            p("mid_b", "ミッドトーン 青", Pd::F(0.0, -100.0, 100.0)),
            p("high_r", "ハイライト 赤", Pd::F(0.0, -100.0, 100.0)),
            p("high_g", "ハイライト 緑", Pd::F(0.0, -100.0, 100.0)),
            p("high_b", "ハイライト 青", Pd::F(0.0, -100.0, 100.0)),
        ]
    ),
    fx!(
        "lut",
        "LUT を適用",
        "カラー補正",
        false,
        [p("path", ".cube ファイル", Pd::S), p("intensity", "適用量（%）", Pd::F(100.0, 0.0, 100.0)),]
    ),
    fx!(
        "tritone",
        "トライトーン",
        "カラー補正",
        false,
        [
            p("highlights", "ハイライト", Pd::C(WHITE)),
            p("midtones", "ミッドトーン", Pd::C([0.5, 0.39, 0.27, 1.0])),
            p("shadows", "シャドウ", Pd::C(BLACK)),
            p("blend", "元の画像とブレンド（%）", Pd::F(0.0, 0.0, 100.0)),
        ]
    ),
    fx!(
        "gradient_map",
        "グラデーションマップ",
        "カラー補正",
        false,
        [
            p("color_a", "暗部の色", Pd::C(BLACK)),
            p("color_b", "明部の色", Pd::C(WHITE)),
            p("blend", "元の画像とブレンド（%）", Pd::F(0.0, 0.0, 100.0)),
        ]
    ),
    fx!(
        "shadow_highlight",
        "シャドウ・ハイライト",
        "カラー補正",
        false,
        [
            p("shadow_amount", "シャドウの量", Pd::F(50.0, 0.0, 100.0)),
            p("highlight_amount", "ハイライトの量", Pd::F(0.0, 0.0, 100.0)),
            p("radius", "半径", Pd::F(30.0, 0.0, 500.0)),
        ]
    ),
    fx!(
        "exposure",
        "露光量",
        "カラー補正",
        false,
        [p("stops", "露出（段）", Pd::F(0.0, -20.0, 20.0)), p("offset", "オフセット", Pd::F(0.0, -1.0, 1.0)),]
    ),
    fx!(
        "brightness_contrast",
        "輝度＆コントラスト",
        "カラー補正",
        false,
        [p("brightness", "輝度", Pd::F(0.0, -150.0, 150.0)), p("contrast", "コントラスト", Pd::F(0.0, -100.0, 100.0)),]
    ),
    fx!("invert", "反転", "カラー補正", false, [p("blend", "元の画像とブレンド（%）", Pd::F(0.0, 0.0, 100.0))]),
    // ---- キーイング ----
    fx!(
        "chroma_key",
        "クロマキー",
        "キーイング",
        false,
        [
            p("key_color", "キーカラー", Pd::C([0.0, 1.0, 0.0, 1.0])),
            p("tolerance", "許容量", Pd::F(0.25, 0.0, 1.0)),
            p("softness", "エッジの柔らかさ", Pd::F(0.1, 0.0, 1.0)),
            p("despill", "スピル除去（%）", Pd::F(100.0, 0.0, 100.0)),
        ]
    ),
    fx!(
        "luma_key",
        "ルミナンスキー",
        "キーイング",
        false,
        [
            p("mode", "キーの種類", Pd::I(0, &["明るい部分をキーアウト", "暗い部分をキーアウト"])),
            p("threshold", "しきい値", Pd::F(0.5, 0.0, 4.0)),
            p("softness", "エッジの柔らかさ", Pd::F(0.1, 0.0, 1.0)),
        ]
    ),
    fx!(
        "matte_choker",
        "マットチョーク",
        "キーイング",
        false,
        [p("choke", "チョーク（px）", Pd::F(0.0, -200.0, 200.0)), p("blur", "ブラー", Pd::F(0.0, 0.0, 200.0)),]
    ),
    // ---- シミュレーション ----
    fx!(
        "particles",
        "パーティクル",
        "シミュレーション",
        true,
        [
            p("emitter", "エミッター", Pd::I(0, &["点", "ボックス", "レイヤー（アルファ）"])),
            p("position", "位置", Pd::P(0.5, 0.5)),
            p("emitter_width", "エミッターの幅", Pd::F(200.0, 0.0, 8000.0)),
            p("emitter_height", "エミッターの高さ", Pd::F(200.0, 0.0, 8000.0)),
            p("emitter_layer", "エミッターレイヤー", Pd::L),
            p("rate", "発生数／秒", Pd::F(100.0, 0.0, 100000.0)),
            p("life", "寿命（秒）", Pd::F(2.0, 0.01, 60.0)),
            p("speed", "速度", Pd::F(200.0, 0.0, 10000.0)),
            p("speed_random", "速度のランダム（%）", Pd::F(30.0, 0.0, 100.0)),
            p("direction", "方向", Pd::F(-90.0, -BIG, BIG)),
            p("spread", "広がり（度）", Pd::F(360.0, 0.0, 360.0)),
            p("gravity", "重力", Pd::F(300.0, -10000.0, 10000.0)),
            p("turbulence", "乱流", Pd::F(0.0, 0.0, 2000.0)),
            p("floor_enabled", "床との衝突", Pd::B(false)),
            p("floor_y", "床の高さ（Y）", Pd::F(1000.0, -BIG, BIG)),
            p("bounce", "反発係数", Pd::F(0.5, 0.0, 1.0)),
            p("size", "サイズ", Pd::F(8.0, 0.1, 2000.0)),
            p("size_end", "終了サイズ（%）", Pd::F(0.0, 0.0, 1000.0)),
            p("color_start", "開始色", Pd::C(WHITE)),
            p("color_end", "終了色", Pd::C([1.0, 0.5, 0.1, 1.0])),
            p("opacity_end", "終了不透明度（%）", Pd::F(0.0, 0.0, 100.0)),
            p("sprite_layer", "スプライトレイヤー", Pd::L),
            p("trail", "トレイル（個）", Pd::F(0.0, 0.0, 64.0)),
            p("additive", "加算合成", Pd::B(true)),
            p("seed", "ランダムシード", Pd::F(0.0, 0.0, 100000.0)),
        ]
    ),
    // ---- エクスプレッション制御 ----
    fx!("slider_control", "スライダー制御", "エクスプレッション制御", false, [p("slider", "スライダー", Pd::F(0.0, -BIG, BIG))]),
    fx!("angle_control", "角度制御", "エクスプレッション制御", false, [p("angle", "角度", Pd::F(0.0, -BIG, BIG))]),
    fx!(
        "checkbox_control",
        "チェックボックス制御",
        "エクスプレッション制御",
        false,
        [p("checkbox", "チェックボックス", Pd::F(0.0, 0.0, 1.0))]
    ),
    fx!("color_control", "カラー制御", "エクスプレッション制御", false, [p("color", "カラー", Pd::C(WHITE))]),
    fx!("point_control", "ポイント制御", "エクスプレッション制御", false, [p("point", "ポイント", Pd::P(0.5, 0.5))]),
    fx!("layer_control", "レイヤー制御", "エクスプレッション制御", false, [p("layer", "レイヤー", Pd::L)]),
    // ---- プラグイン ----
    fx!(
        "plugin",
        "外部プラグイン",
        "プラグイン",
        true,
        [
            p("path", "プラグインファイル", Pd::S),
            p("a", "パラメータ A", Pd::F(0.0, -BIG, BIG)),
            p("b", "パラメータ B", Pd::F(0.0, -BIG, BIG)),
            p("c", "パラメータ C", Pd::F(0.0, -BIG, BIG)),
            p("d", "パラメータ D", Pd::F(0.0, -BIG, BIG)),
        ]
    ),
];

/// 実行時に登録されたエフェクト（プラグイン）。
fn dynamic() -> &'static std::sync::RwLock<Vec<&'static EffectDef>> {
    static DYNAMIC: std::sync::OnceLock<std::sync::RwLock<Vec<&'static EffectDef>>> = std::sync::OnceLock::new();
    DYNAMIC.get_or_init(Default::default)
}

/// プラグインが提供するパラメータの定義（実行時に決まる）。
#[derive(Debug, Clone, PartialEq)]
pub enum DynPd {
    F(f64, f64, f64),
    P(f64, f64),
    C([f64; 4]),
    B(bool),
    I(u32, Vec<String>),
    S,
    /// 他のレイヤーの参照。
    L,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DynEffect {
    pub kind: String,
    pub label: String,
    pub category: String,
    pub time_dependent: bool,
    /// (名前, 表示名, 定義)
    pub params: Vec<(String, String, DynPd)>,
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

/// プラグインのエフェクトを定義表に加える。同じ `kind` が既にあれば何もしない
/// （定義はプロセスが終わるまで保持する。プラグインの数だけなので量は小さい）。
pub fn register_effect(def: DynEffect) -> &'static EffectDef {
    if let Some(existing) = effect_def(&def.kind) {
        return existing;
    }
    let params: Vec<ParamDef> = def
        .params
        .iter()
        .map(|(name, label, pd)| ParamDef {
            name: leak(name),
            label: leak(label),
            kind: match pd {
                DynPd::F(d, lo, hi) => Pd::F(*d, *lo, *hi),
                DynPd::P(x, y) => Pd::P(*x, *y),
                DynPd::C(c) => Pd::C(*c),
                DynPd::B(b) => Pd::B(*b),
                DynPd::I(d, labels) => {
                    let labels: Vec<&'static str> = labels.iter().map(|l| leak(l)).collect();
                    Pd::I(*d, Box::leak(labels.into_boxed_slice()))
                }
                DynPd::S => Pd::S,
                DynPd::L => Pd::L,
            },
        })
        .collect();
    let leaked: &'static EffectDef = Box::leak(Box::new(EffectDef {
        kind: leak(&def.kind),
        label: leak(&def.label),
        category: leak(&def.category),
        time_dependent: def.time_dependent,
        params: Box::leak(params.into_boxed_slice()),
    }));
    dynamic().write().unwrap().push(leaked);
    leaked
}

pub fn effect_def(kind: &str) -> Option<&'static EffectDef> {
    EFFECTS.iter().find(|d| d.kind == kind).or_else(|| dynamic().read().unwrap().iter().copied().find(|d| d.kind == kind))
}

/// 組み込みと、登録済みのプラグインのすべてのエフェクト。
pub fn all_effects() -> Vec<&'static EffectDef> {
    EFFECTS.iter().chain(dynamic().read().unwrap().iter().copied()).collect()
}

// ---------- 音声エフェクト（プラグインが提供する） ----------

/// 音声エフェクトに渡す 1 ブロック。
pub struct AudioBlock<'a> {
    /// ステレオのインターリーブ（L, R, L, R, ...）、48kHz。その場で書き換える。
    pub samples: &'a mut [f32],
    pub sample_rate: u32,
    /// `samples[0]` の、レイヤーのイン点から数えたサンプル位置。
    pub start: i64,
    /// `samples[0]` のコンポ上の時刻。
    pub time: Time,
    /// エフェクトの実体ごとに異なる値（レイヤー ID とエフェクトの位置から作る）。状態を持つ処理の区別に使う。
    pub instance: u64,
    /// レイヤーごとの乱数の種。
    pub seed: u32,
    /// レイヤーの長さ（サンプル数、イン点からアウト点まで）。全体を測ってから掛ける処理（ラウドネスなど）に使う。
    pub length: i64,
}

/// 音声エフェクトの処理本体。`znd-audio` のミキサーが、レイヤーの音に順に掛ける。
pub trait AudioProcessor: Send + Sync {
    fn process(&self, params: &EvalParams, block: AudioBlock);
}

fn audio_processors() -> &'static std::sync::RwLock<BTreeMap<String, std::sync::Arc<dyn AudioProcessor>>> {
    static P: std::sync::OnceLock<std::sync::RwLock<BTreeMap<String, std::sync::Arc<dyn AudioProcessor>>>> = std::sync::OnceLock::new();
    P.get_or_init(Default::default)
}

/// `kind` のエフェクトを音声エフェクトとして登録する（映像には掛からない）。
pub fn register_audio_processor(kind: &str, p: std::sync::Arc<dyn AudioProcessor>) {
    audio_processors().write().unwrap().insert(kind.to_string(), p);
}

/// `kind` が音声エフェクトなら、その処理本体。
pub fn audio_processor(kind: &str) -> Option<std::sync::Arc<dyn AudioProcessor>> {
    audio_processors().read().unwrap().get(kind).cloned()
}

/// エフェクトプリセット（複数エフェクトの束）。パラメータ・キーフレーム・エクスプレッションを含む。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectPreset {
    pub name: String,
    pub effects: Vec<EffectInstance>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_are_unique_and_instantiable() {
        for (i, d) in EFFECTS.iter().enumerate() {
            assert!(!EFFECTS[..i].iter().any(|o| o.kind == d.kind), "duplicate kind {}", d.kind);
            let fx = EffectInstance::new(d.kind, 1920.0, 1080.0).unwrap();
            assert_eq!(fx.params.len(), d.params.len(), "{}: duplicate param names", d.kind);
            let _ = fx.eval(Time::ZERO);
        }
        assert!(EffectInstance::new("nope", 1.0, 1.0).is_none());
    }

    #[test]
    fn plugin_effects_can_be_registered_at_runtime() {
        let def = DynEffect {
            kind: "plugin:test.tint".into(),
            label: "色かぶり".into(),
            category: "プラグイン".into(),
            time_dependent: false,
            params: vec![
                ("amount".into(), "量".into(), DynPd::F(0.5, 0.0, 1.0)),
                ("mode".into(), "方式".into(), DynPd::I(1, vec!["加算".into(), "乗算".into()])),
                ("center".into(), "中心".into(), DynPd::P(0.5, 0.25)),
            ],
        };
        let a = register_effect(def.clone());
        // 二重登録しても定義は 1 つ
        assert!(std::ptr::eq(a, register_effect(def)));
        assert_eq!(all_effects().iter().filter(|d| d.kind == "plugin:test.tint").count(), 1);
        let fx = EffectInstance::new("plugin:test.tint", 200.0, 100.0).unwrap();
        let p = fx.eval(Time::ZERO);
        assert_eq!((p.f("amount"), p.i("mode"), p.p("center")), (0.5, 1, [100.0, 25.0]));
    }

    #[test]
    fn preset_roundtrip_keeps_keyframes_and_expressions() {
        use crate::property::{Expression, Keyframe};
        let mut blur = EffectInstance::new("gaussian_blur", 100.0, 100.0).unwrap();
        let mut radius =
            Property::constant(0.0).with_keys(vec![Keyframe::linear(Time::ZERO, 0.0), Keyframe::easy_ease(Time::from_seconds(1.0), 40.0)]);
        radius.expression = Some(Expression { source: "value * 2".into(), enabled: true });
        blur = blur.set_prop("radius", radius);
        let preset = EffectPreset { name: "ふわっと".into(), effects: vec![blur] };
        let json = serde_json::to_string_pretty(&preset).unwrap();
        assert_eq!(serde_json::from_str::<EffectPreset>(&json).unwrap(), preset);
        assert_eq!(preset.effects[0].eval(Time::from_seconds(1.0)).f("radius"), 40.0);
    }
}
