//! プロジェクトのデータモデル。JSON にそのままシリアライズされる。
//!
//! 差分管理しやすいよう、順序を持つものは `Vec`、既定値のフィールドは省略、
//! 参照は文字列 ID で行う。

use crate::beat::BeatGrid;
use crate::effect::EffectInstance;
use crate::property::Property;
use crate::shape::{Mask, ShapeItem};
use crate::text::{TextAnimator, TextDocument, TextPath};
use crate::time::{FrameRate, Time};
use serde::{Deserialize, Serialize};

/// プロジェクトファイル形式のバージョン。互換性のない変更で上げる。
pub const FORMAT_VERSION: u32 = 1;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

id_type!(CompId);
id_type!(LayerId);
id_type!(FootageId);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub format_version: u32,
    pub name: String,
    #[serde(default)]
    pub settings: ProjectSettings,
    #[serde(default)]
    pub footage: Vec<FootageItem>,
    pub compositions: Vec<Composition>,
    /// フレーム指定のレビューコメント（共同制作用）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reviews: Vec<ReviewComment>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewComment {
    pub comp: CompId,
    pub time: Time,
    pub author: String,
    pub text: String,
    /// 画面上の注釈位置（コンポ座標）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point: Option<[f64; 2]>,
    #[serde(default)]
    pub resolved: bool,
    /// 記入日時（ISO 8601）。
    #[serde(default)]
    pub created: String,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorSpace {
    #[default]
    Srgb,
    Rec709,
    DisplayP3,
    /// ACEScg（AP1 原色）。出力先としてはリニアのまま書き出す。
    AcesCg,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BitDepth {
    #[serde(rename = "8")]
    B8,
    #[serde(rename = "16")]
    B16,
    #[default]
    #[serde(rename = "32f")]
    F32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectSettings {
    /// true: リニアライトで合成（既定）。false: 表示ガンマ空間で合成（従来ツールの既定に合わせたい場合）。
    pub linear_blending: bool,
    /// 色深度。8/16 では各レイヤー合成後に 0..1 へ収め、その段数に量子化する。
    pub bit_depth: BitDepth,
    /// 作業色空間（原色）。合成は常にこの原色のリニア値で行う。
    pub working_space: ColorSpace,
    /// 出力（表示・書き出し）色空間。
    pub output_space: ColorSpace,
}

impl Default for ProjectSettings {
    fn default() -> Self {
        ProjectSettings { linear_blending: true, bit_depth: BitDepth::F32, working_space: ColorSpace::Srgb, output_space: ColorSpace::Srgb }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FootageItem {
    pub id: FootageId,
    pub name: String,
    /// プロジェクトファイルからの相対パス、または絶対パス。
    pub path: String,
    pub width: u32,
    pub height: u32,
    /// 静止画は `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_rate: Option<FrameRate>,
    /// 静止画は `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<Time>,
    #[serde(default, skip_serializing_if = "Rights::is_empty")]
    pub rights: Rights,
    /// 音声ストリームの情報（無ければ `None`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioInfo>,
    /// 可変フレームレート素材。デコード時に `frame_rate` の固定レートへ適合させる。
    #[serde(default, skip_serializing_if = "is_false")]
    pub vfr: bool,
}

#[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioInfo {
    pub sample_rate: u32,
    pub channels: u32,
    pub duration: Time,
}

/// 権利メタデータ。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Rights {
    pub author: String,
    pub source_url: String,
    pub license: String,
    pub copyright: Clearance,
    /// 著作隣接権・原盤権。
    pub neighboring: Clearance,
    pub portrait: Clearance,
    pub trademark: Clearance,
    /// 許諾日（ISO 8601）。
    pub cleared_on: String,
    pub notes: String,
}

impl Rights {
    pub fn is_empty(&self) -> bool {
        *self == Rights::default()
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Clearance {
    /// 許諾未確認。
    #[default]
    Unconfirmed,
    Cleared,
    /// 自作・該当なし。
    NotApplicable,
    Denied,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Composition {
    pub id: CompId,
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// ピクセル縦横比。レンダラーは 1.0 として扱う（保存のみ）。
    #[serde(default = "one")]
    pub pixel_aspect: f64,
    pub frame_rate: FrameRate,
    pub duration: Time,
    /// 背景色（sRGB 表示値）。ネスト時とアルファ出力時は使われない。
    #[serde(default)]
    pub background: [f32; 3],
    /// 先頭が最前面。
    pub layers: Vec<Layer>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beat_grid: Option<BeatGrid>,
    #[serde(default, skip_serializing_if = "MotionBlur::is_default")]
    pub motion_blur: MotionBlur,
    /// 共同制作用のロック。設定した人の名前。他の人はこのコンポを編集できない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked_by: Option<String>,
}

/// コンポ全体のモーションブラー設定。レイヤーごとのスイッチが入ったものにだけ掛かる。
#[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MotionBlur {
    pub enabled: bool,
    /// シャッター角（度）。180 で 1/2 フレームぶん露光。
    pub shutter_angle: f64,
    /// シャッター位相（度）。-90 でフレーム時刻を中心に露光。
    pub shutter_phase: f64,
    pub samples: u32,
}

impl Default for MotionBlur {
    fn default() -> Self {
        MotionBlur { enabled: false, shutter_angle: 180.0, shutter_phase: -90.0, samples: 16 }
    }
}

impl MotionBlur {
    pub fn is_default(&self) -> bool {
        *self == MotionBlur::default()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub time: Time,
    #[serde(default)]
    pub duration: Time,
    #[serde(default)]
    pub comment: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub kind: LayerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<LayerId>,
    /// コンポ時間でのイン点（含む）。
    pub in_point: Time,
    /// コンポ時間でのアウト点（含まない）。
    pub out_point: Time,
    /// ソースの時刻 0 が来るコンポ時間。
    #[serde(default)]
    pub start_time: Time,
    pub transform: Transform,
    #[serde(default)]
    pub blend_mode: BlendMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_matte: Option<TrackMatte>,
    #[serde(default = "yes")]
    pub visible: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub solo: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub shy: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub locked: bool,
    /// ガイドレイヤー。プレビューには出るが書き出しには含めない。
    #[serde(default, skip_serializing_if = "is_false")]
    pub guide: bool,
    /// コラップス・トランスフォーム（プリコンポのみ有効）。
    #[serde(default, skip_serializing_if = "is_false")]
    pub collapse: bool,
    /// カラーラベル番号（0 = なし）。
    #[serde(default)]
    pub label: u8,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
    /// タイムリマップ（コンポ時刻 → ソース秒）。あれば `start_time`・`speed` より優先。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_remap: Option<Property<f64>>,
    /// 再生速度（1.0 = 等速）。映像と音声の両方に掛かる（タイムストレッチ）。
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub speed: f64,
    /// コマ打ち: ソース時刻をコンポの N フレームごとに保持する（1 = なし、2 = 2 コマ打ち）。
    #[serde(default = "one_u32", skip_serializing_if = "is_one_u32")]
    pub hold_frames: u32,
    /// フレームブレンド（フレームミックス）。
    #[serde(default, skip_serializing_if = "is_false")]
    pub frame_blend: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub masks: Vec<Mask>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<EffectInstance>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub motion_blur: bool,
    /// 3D レイヤー。
    #[serde(default, skip_serializing_if = "is_false")]
    pub three_d: bool,
    /// ライトの影響を受ける（3D レイヤーのみ）。
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub accepts_lights: bool,
    /// 影を落とす（3D レイヤーのみ）。
    #[serde(default, skip_serializing_if = "is_false")]
    pub casts_shadows: bool,
    #[serde(default, skip_serializing_if = "AudioProps::is_default")]
    pub audio: AudioProps,
}

/// レイヤーの音声設定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioProps {
    /// 音量（dB）。
    pub level_db: Property<f64>,
    pub mute: bool,
    /// クリップ先頭・末尾のフェード。
    pub fade_in: Time,
    pub fade_out: Time,
    /// ピッチシフト（半音）。尺は変えない。
    pub pitch: f64,
}

impl Default for AudioProps {
    fn default() -> Self {
        AudioProps { level_db: Property::constant(0.0), mute: false, fade_in: Time::ZERO, fade_out: Time::ZERO, pitch: 0.0 }
    }
}

impl AudioProps {
    pub fn is_default(&self) -> bool {
        *self == AudioProps::default()
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LightType {
    Parallel,
    Spot,
    #[default]
    Point,
    Ambient,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LayerKind {
    /// 平面。色は sRGB 表示値。
    Solid {
        width: u32,
        height: u32,
        color: [f32; 3],
    },
    Footage {
        footage: FootageId,
    },
    Precomp {
        comp: CompId,
    },
    Null,
    /// 調整レイヤー。自身のエフェクトを、下にある合成結果に掛ける。
    Adjustment,
    Shape {
        items: Vec<ShapeItem>,
    },
    Text {
        document: TextDocument,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        animators: Vec<TextAnimator>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<TextPath>,
        /// 文字ごとの 3D（アニメーターの Z 位置・X/Y 回転を有効にする）。
        #[serde(default, skip_serializing_if = "is_false")]
        per_char_3d: bool,
    },
    /// 音声のみのレイヤー。
    Audio {
        footage: FootageId,
    },
    Camera {
        /// ズーム（px）。カメラから「等倍に見える面」までの距離。
        zoom: Property<f64>,
        /// 目標点（2 ノードカメラ）。`None` なら回転で向きを決める 1 ノードカメラ。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        point_of_interest: Option<Property<[f64; 3]>>,
        #[serde(default)]
        depth_of_field: bool,
        focus_distance: Property<f64>,
        /// 絞り（px）。大きいほどボケる。
        aperture: Property<f64>,
        /// 絞りの形状（羽根枚数。0 で円形）。
        #[serde(default)]
        iris_sides: u32,
    },
    Light {
        #[serde(default)]
        light: LightType,
        color: [f32; 3],
        /// 強度（%）。
        intensity: Property<f64>,
        cone_angle: Property<f64>,
        cone_feather: Property<f64>,
        #[serde(default)]
        casts_shadows: bool,
        /// 影の暗さ（%）。
        #[serde(default = "hundred")]
        shadow_darkness: f64,
        /// 影の拡散（px）。
        #[serde(default)]
        shadow_diffusion: f64,
    },
    /// 3D モデル（glTF / OBJ）。フッテージ項目がモデルファイルを指す。
    Model {
        footage: FootageId,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    pub anchor: Property<[f64; 2]>,
    pub position: Property<[f64; 2]>,
    /// パーセント。
    pub scale: Property<[f64; 2]>,
    /// 度。
    pub rotation: Property<f64>,
    /// 0..100。
    pub opacity: Property<f64>,
    // ---- 以下は 3D レイヤー・カメラ・ライトで使う ----
    #[serde(default = "zero_prop", skip_serializing_if = "is_zero_prop")]
    pub position_z: Property<f64>,
    #[serde(default = "zero_prop", skip_serializing_if = "is_zero_prop")]
    pub anchor_z: Property<f64>,
    #[serde(default = "zero_prop", skip_serializing_if = "is_zero_prop")]
    pub rotation_x: Property<f64>,
    #[serde(default = "zero_prop", skip_serializing_if = "is_zero_prop")]
    pub rotation_y: Property<f64>,
}

fn zero_prop() -> Property<f64> {
    Property::constant(0.0)
}

fn is_zero_prop(p: &Property<f64>) -> bool {
    *p == zero_prop()
}

impl Transform {
    /// ソースの中心をアンカーにしてコンポ中央に置く。
    pub fn centered(src_w: f64, src_h: f64, comp_w: f64, comp_h: f64) -> Self {
        Transform {
            anchor: Property::constant([src_w / 2.0, src_h / 2.0]),
            position: Property::spatial([comp_w / 2.0, comp_h / 2.0]),
            scale: Property::constant([100.0, 100.0]),
            rotation: Property::constant(0.0),
            opacity: Property::constant(100.0),
            position_z: zero_prop(),
            anchor_z: zero_prop(),
            rotation_x: zero_prop(),
            rotation_y: zero_prop(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackMatte {
    pub layer: LayerId,
    pub mode: MatteMode,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatteMode {
    Alpha,
    AlphaInverted,
    Luma,
    LumaInverted,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
    #[default]
    Normal,
    Darken,
    Multiply,
    ColorBurn,
    LinearBurn,
    DarkerColor,
    Add,
    Lighten,
    Screen,
    ColorDodge,
    LighterColor,
    Overlay,
    SoftLight,
    HardLight,
    LinearLight,
    VividLight,
    PinLight,
    HardMix,
    Difference,
    Exclusion,
    Subtract,
    Divide,
    Hue,
    Saturation,
    Color,
    Luminosity,
    StencilAlpha,
    StencilLuma,
    SilhouetteAlpha,
    SilhouetteLuma,
    AlphaAdd,
}

impl BlendMode {
    pub const ALL: [BlendMode; 31] = [
        BlendMode::Normal,
        BlendMode::Darken,
        BlendMode::Multiply,
        BlendMode::ColorBurn,
        BlendMode::LinearBurn,
        BlendMode::DarkerColor,
        BlendMode::Add,
        BlendMode::Lighten,
        BlendMode::Screen,
        BlendMode::ColorDodge,
        BlendMode::LighterColor,
        BlendMode::Overlay,
        BlendMode::SoftLight,
        BlendMode::HardLight,
        BlendMode::LinearLight,
        BlendMode::VividLight,
        BlendMode::PinLight,
        BlendMode::HardMix,
        BlendMode::Difference,
        BlendMode::Exclusion,
        BlendMode::Subtract,
        BlendMode::Divide,
        BlendMode::Hue,
        BlendMode::Saturation,
        BlendMode::Color,
        BlendMode::Luminosity,
        BlendMode::StencilAlpha,
        BlendMode::StencilLuma,
        BlendMode::SilhouetteAlpha,
        BlendMode::SilhouetteLuma,
        BlendMode::AlphaAdd,
    ];

    pub fn label_ja(self) -> &'static str {
        match self {
            BlendMode::Normal => "通常",
            BlendMode::Darken => "比較（暗）",
            BlendMode::Multiply => "乗算",
            BlendMode::ColorBurn => "焼き込みカラー",
            BlendMode::LinearBurn => "焼き込みリニア",
            BlendMode::DarkerColor => "カラー比較（暗）",
            BlendMode::Add => "加算",
            BlendMode::Lighten => "比較（明）",
            BlendMode::Screen => "スクリーン",
            BlendMode::ColorDodge => "覆い焼きカラー",
            BlendMode::LighterColor => "カラー比較（明）",
            BlendMode::Overlay => "オーバーレイ",
            BlendMode::SoftLight => "ソフトライト",
            BlendMode::HardLight => "ハードライト",
            BlendMode::LinearLight => "リニアライト",
            BlendMode::VividLight => "ビビッドライト",
            BlendMode::PinLight => "ピンライト",
            BlendMode::HardMix => "ハードミックス",
            BlendMode::Difference => "差の絶対値",
            BlendMode::Exclusion => "除外",
            BlendMode::Subtract => "減算",
            BlendMode::Divide => "除算",
            BlendMode::Hue => "色相",
            BlendMode::Saturation => "彩度",
            BlendMode::Color => "カラー",
            BlendMode::Luminosity => "輝度",
            BlendMode::StencilAlpha => "ステンシルアルファ",
            BlendMode::StencilLuma => "ステンシルルミナンス",
            BlendMode::SilhouetteAlpha => "シルエットアルファ",
            BlendMode::SilhouetteLuma => "シルエットルミナンス",
            BlendMode::AlphaAdd => "アルファ追加",
        }
    }
}

fn one() -> f64 {
    1.0
}
fn yes() -> bool {
    true
}
fn is_true(b: &bool) -> bool {
    *b
}
fn is_one(v: &f64) -> bool {
    *v == 1.0
}
fn one_u32() -> u32 {
    1
}
fn is_one_u32(v: &u32) -> bool {
    *v == 1
}
fn hundred() -> f64 {
    100.0
}
fn is_false(b: &bool) -> bool {
    !*b
}

impl Layer {
    pub fn new(id: impl Into<String>, name: impl Into<String>, kind: LayerKind, transform: Transform, duration: Time) -> Self {
        Layer {
            id: LayerId::new(id),
            name: name.into(),
            kind,
            parent: None,
            in_point: Time::ZERO,
            out_point: duration,
            start_time: Time::ZERO,
            transform,
            blend_mode: BlendMode::Normal,
            track_matte: None,
            visible: true,
            solo: false,
            shy: false,
            locked: false,
            guide: false,
            collapse: false,
            label: 0,
            markers: Vec::new(),
            time_remap: None,
            speed: 1.0,
            hold_frames: 1,
            frame_blend: false,
            masks: Vec::new(),
            effects: Vec::new(),
            motion_blur: false,
            three_d: false,
            accepts_lights: true,
            casts_shadows: false,
            audio: AudioProps::default(),
        }
    }

    pub fn active_at(&self, t: Time) -> bool {
        t >= self.in_point && t < self.out_point
    }
}

impl Composition {
    pub fn new(id: impl Into<String>, name: impl Into<String>, width: u32, height: u32, frame_rate: FrameRate, duration: Time) -> Self {
        Composition {
            id: CompId::new(id),
            name: name.into(),
            width,
            height,
            pixel_aspect: 1.0,
            frame_rate,
            duration,
            background: [0.0; 3],
            layers: Vec::new(),
            markers: Vec::new(),
            beat_grid: None,
            motion_blur: MotionBlur::default(),
            locked_by: None,
        }
    }

    /// 構造体更新構文（`..Composition::blank()`）用の空のコンポ。
    pub fn blank() -> Self {
        Composition::new("", "", 0, 0, FrameRate::FPS_30, Time::ZERO)
    }

    pub fn layer_index(&self, id: &LayerId) -> Option<usize> {
        self.layers.iter().position(|l| &l.id == id)
    }

    pub fn layer(&self, id: &LayerId) -> Option<&Layer> {
        self.layers.iter().find(|l| &l.id == id)
    }

    pub fn layer_mut(&mut self, id: &LayerId) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|l| &l.id == id)
    }

    pub fn frame_count(&self) -> i64 {
        self.frame_rate.frame_count(self.duration)
    }
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        Project {
            format_version: FORMAT_VERSION,
            name: name.into(),
            settings: ProjectSettings::default(),
            footage: Vec::new(),
            compositions: Vec::new(),
            reviews: Vec::new(),
        }
    }

    pub fn comp(&self, id: &CompId) -> Option<&Composition> {
        self.compositions.iter().find(|c| &c.id == id)
    }

    pub fn comp_mut(&mut self, id: &CompId) -> Option<&mut Composition> {
        self.compositions.iter_mut().find(|c| &c.id == id)
    }

    pub fn footage_item(&self, id: &FootageId) -> Option<&FootageItem> {
        self.footage.iter().find(|f| &f.id == id)
    }

    /// レイヤーのソース寸法（ヌル・調整レイヤーは `None`）。
    pub fn source_size(&self, kind: &LayerKind) -> Option<(f64, f64)> {
        match kind {
            LayerKind::Solid { width, height, .. } => Some((*width as f64, *height as f64)),
            LayerKind::Footage { footage } => self.footage_item(footage).map(|f| (f.width as f64, f.height as f64)),
            LayerKind::Precomp { comp } => self.comp(comp).map(|c| (c.width as f64, c.height as f64)),
            _ => None,
        }
    }

    /// プロジェクト内で未使用の ID を `prefix-N` 形式で返す。
    pub fn fresh_id(&self, prefix: &str) -> String {
        let used = |s: &str| {
            self.footage.iter().any(|f| f.id.0 == s)
                || self.compositions.iter().any(|c| c.id.0 == s || c.layers.iter().any(|l| l.id.0 == s))
        };
        (1..).map(|n| format!("{prefix}-{n}")).find(|s| !used(s)).unwrap()
    }

    /// 参照切れ・循環などの構造エラーを列挙する。空なら健全。
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();
        for comp in &self.compositions {
            if comp.width == 0 || comp.height == 0 {
                issues.push(format!("コンポ「{}」: 解像度が 0 です", comp.name));
            }
            for (i, layer) in comp.layers.iter().enumerate() {
                let at = format!("コンポ「{}」レイヤー「{}」", comp.name, layer.name);
                if comp.layers[..i].iter().any(|l| l.id == layer.id) {
                    issues.push(format!("{at}: レイヤー ID「{}」が重複しています", layer.id));
                }
                match &layer.kind {
                    LayerKind::Footage { footage } | LayerKind::Audio { footage } | LayerKind::Model { footage }
                        if self.footage_item(footage).is_none() =>
                    {
                        issues.push(format!("{at}: フッテージ「{footage}」が見つかりません"));
                    }
                    LayerKind::Precomp { comp: c } if self.comp(c).is_none() => {
                        issues.push(format!("{at}: コンポ「{c}」が見つかりません"));
                    }
                    _ => {}
                }
                if let Some(m) = &layer.track_matte {
                    if comp.layer(&m.layer).is_none() {
                        issues.push(format!("{at}: トラックマット参照「{}」が見つかりません", m.layer));
                    }
                }
                // 親をたどって循環と参照切れを検出
                let mut cur = layer.parent.as_ref();
                let mut steps = 0;
                while let Some(pid) = cur {
                    match comp.layer(pid) {
                        None => {
                            issues.push(format!("{at}: 親「{pid}」が見つかりません"));
                            break;
                        }
                        Some(p) => cur = p.parent.as_ref(),
                    }
                    steps += 1;
                    if steps > comp.layers.len() {
                        issues.push(format!("{at}: 親子関係が循環しています"));
                        break;
                    }
                }
            }
            if self.comp_reaches(&comp.id, &comp.id, 0) {
                issues.push(format!("コンポ「{}」: プリコンポが循環しています", comp.name));
            }
        }
        issues
    }

    /// `from` のプリコンポをたどって `target` に到達するか。
    fn comp_reaches(&self, from: &CompId, target: &CompId, depth: usize) -> bool {
        if depth > self.compositions.len() {
            return true;
        }
        let Some(comp) = self.comp(from) else {
            return false;
        };
        comp.layers.iter().any(|l| match &l.kind {
            LayerKind::Precomp { comp: c } => c == target || self.comp_reaches(c, target, depth + 1),
            _ => false,
        })
    }
}
