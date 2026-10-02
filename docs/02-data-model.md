# 2. データモデル

実装は `crates/znd-core/src/{time,property,model}.rs`。ここに載せる型はプロジェクトファイル（JSON）と 1:1 に対応する。
下のコードは M1 時点の骨格。`(M2)` などの印が付いた項目は、その後のマイルストーンで**追加済み**（例外は下の注記）。
正確な定義は [model.rs](../crates/znd-core/src/model.rs) を参照。

- 未実装のまま: `audio_analysis` のキャッシュ参照（解析はその都度行う）、`Dissolve` 系・`Classic*`・`LuminescentPremul` の描画モード。
- 形が変わったもの: スナップショットと制作ログはプロジェクト JSON の中ではなく、プロジェクトファイルの隣に別ファイルで保存する。
  コマ打ちは `hold_frames`、プロキシはプロジェクトの隣の `.znd-proxy/` に置く。3D は `Transform` に `position_z` / `anchor_z` / `rotation_x` / `rotation_y` を足す形。

## 時刻

```rust
/// flicks（1/705,600,000 秒）。全フレームレートと 44.1/48/96kHz を割り切る。
struct Time(i64);

/// 有理数。23.976fps = 24000/1001。
struct FrameRate { num: u32, den: u32 }
// プリセット: 23.976 / 24 / 25 / 29.97 / 30 / 50 / 59.94 / 60
```

## プロジェクト

```rust
struct Project {
    format_version: u32,              // 現在 1。新しい版のファイルは開かずにエラーにする
    name: String,
    settings: ProjectSettings,
    footage: Vec<FootageItem>,
    compositions: Vec<Composition>,
    // (M2) audio_analysis: ビート・帯域音量のキャッシュ参照
    // (M5) snapshots: 「1次提出版」等の版, reviews: フレーム指定コメント, production_log
}

struct ProjectSettings {
    linear_blending: bool,            // 既定 true
    // (M3) bit_depth: 8 | 16 | 32f,  working_space / output_space: sRGB | Rec709 | DisplayP3 | ACEScg
}

struct FootageItem {
    id: FootageId,
    name: String,
    path: String,                     // プロジェクトファイルからの相対 or 絶対
    width: u32, height: u32,
    frame_rate: Option<FrameRate>,    // 静止画は None
    duration: Option<Time>,
    rights: Rights,
    // (M2) proxy, audio: { sample_rate, channels }, vfr_map
}

/// 権利メタデータ。フォント・音源・プラグインにも同じ型を付ける (M5)。
struct Rights {
    author: String,
    source_url: String,
    license: String,
    copyright: Clearance,
    neighboring: Clearance,           // 著作隣接権・原盤権
    portrait: Clearance,
    trademark: Clearance,
    cleared_on: String,               // ISO 8601
    notes: String,
}
enum Clearance { Unconfirmed, Cleared, NotApplicable, Denied }   // 既定は Unconfirmed（未確認）
```

## コンポジションとレイヤー

```rust
struct Composition {
    id: CompId,
    name: String,
    width: u32, height: u32,
    pixel_aspect: f64,
    frame_rate: FrameRate,
    duration: Time,
    background: [f32; 3],             // sRGB 表示値
    layers: Vec<Layer>,               // 先頭が最前面
    markers: Vec<Marker>,
    // (M2) beat_grid: { bpm, meter, offset }
    // (M3) motion_blur: { shutter_angle, shutter_phase, samples }
    // (M5) locked_by: 共同制作用のコンポ単位ロック
}

struct Marker { time: Time, duration: Time, comment: String }

struct Layer {
    id: LayerId,
    name: String,
    kind: LayerKind,
    parent: Option<LayerId>,
    in_point: Time,                   // 含む
    out_point: Time,                  // 含まない
    start_time: Time,                 // ソース時刻 0 が来るコンポ時刻
    transform: Transform,
    blend_mode: BlendMode,
    track_matte: Option<TrackMatte>,  // 任意レイヤー参照
    visible: bool, solo: bool, shy: bool, locked: bool, guide: bool,
    collapse: bool,                   // コラップス・トランスフォーム
    label: u8,                        // カラーラベル
    markers: Vec<Marker>,
    // (M2) time_remap: Property<f64>, frame_blend, posterize_time, masks: Vec<Mask>, audio
    // (M3) effects: Vec<EffectInstance>, motion_blur: bool
    // (M4) three_d: bool
}

enum LayerKind {
    Solid { width, height, color },
    Footage { footage: FootageId },
    Precomp { comp: CompId },
    Null,
    Adjustment,
    // (M2) Shape { contents }, Text { document, animators }, Audio { footage }
    // (M4) Camera { .. }, Light { .. }, Model { .. }
}

struct Transform {
    anchor:   Property<[f64; 2]>,
    position: Property<[f64; 2]>,     // 空間プロパティ
    scale:    Property<[f64; 2]>,     // %
    rotation: Property<f64>,          // 度
    opacity:  Property<f64>,          // 0..100
    // (M4) 3D レイヤーでは [f64; 3] と orientation / x,y,z rotation に拡張
}

struct TrackMatte { layer: LayerId, mode: MatteMode }
enum MatteMode { Alpha, AlphaInverted, Luma, LumaInverted }

enum BlendMode {   // 31 種を実装
    Normal, Darken, Multiply, ColorBurn, LinearBurn, DarkerColor,
    Add, Lighten, Screen, ColorDodge, LighterColor,
    Overlay, SoftLight, HardLight, LinearLight, VividLight, PinLight, HardMix,
    Difference, Exclusion, Subtract, Divide,
    Hue, Saturation, Color, Luminosity,
    StencilAlpha, StencilLuma, SilhouetteAlpha, SilhouetteLuma, AlphaAdd,
    // (M3) Dissolve, DancingDissolve, ClassicColorBurn/Dodge, ClassicDifference, LuminescentPremul
}
```

## プロパティ・キーフレーム・エクスプレッション

```rust
/// 値の型は f64 / [f64; 2] / [f64; 3]（Animatable）。色・パス・テキスト文書も同じ仕組みで扱う。
struct Property<V> {
    value: V,                         // キーが無いときの値
    keyframes: Vec<Keyframe<V>>,      // 時刻昇順
    spatial: bool,                    // モーションパスを持つ（位置など）
    expression: Option<Expression>,
}

struct Keyframe<V> {
    time: Time,
    value: V,
    interp_in: Interp,                // Linear | Bezier | Hold
    interp_out: Interp,
    ease_in: Vec<Ease>,               // 次元ごと（空間プロパティは 1 要素）
    ease_out: Vec<Ease>,
    bezier_mode: BezierMode,          // Manual（ベジェ）| Continuous（連続ベジェ）| Auto（自動ベジェ）
    spatial: Option<SpatialTangents>,
    // (M2) roving: bool
}

/// After Effects と同じ「速度＋影響度」モデル。グラフエディターの数値入力はこの 2 値を直接編集する。
struct Ease { speed: f64 /* 単位/秒 */, influence: f64 /* 0..1 */ }

struct SpatialTangents { tan_in: Vec<f64>, tan_out: Vec<f64>, auto: bool }

struct Expression {
    source: String,                   // JavaScript
    enabled: bool,
    // 評価は znd-script（QuickJS）。エラー時はキーフレーム値にフォールバックし、行番号付きで報告する
}
```

## ID と参照

- `CompId` / `LayerId` / `FootageId` は文字列。レイヤー ID はコンポ内で一意。
- 参照（親・トラックマット・プリコンポ・フッテージ）はすべて ID。`Project::validate()` が参照切れ・
  親子の循環・プリコンポの循環を列挙し、アプリは循環を作る操作を拒否する。レンダラーも循環で
  無限再帰せずエラーを返す。

## JSON の例

```json
{
  "id": "mover",
  "name": "ムーバー",
  "kind": { "type": "solid", "width": 300, "height": 300, "color": [0.95, 0.2, 0.7] },
  "in_point": 0,
  "out_point": 3528000000,
  "transform": {
    "position": {
      "value": [960.0, 540.0],
      "keyframes": [
        { "time": 0, "value": [300.0, 800.0], "interp_in": "bezier", "interp_out": "bezier",
          "bezier_mode": "manual", "spatial": { "tan_in": [], "tan_out": [], "auto": true } }
      ],
      "spatial": true
    }
  }
}
```

時刻が flicks の整数（5 秒 = 3528000000）で人には読みにくい点は、正確さを優先した意図的な選択。
`znd info` とアプリがフレーム番号・秒で表示する。
