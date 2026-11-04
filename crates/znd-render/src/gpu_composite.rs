//! 合成の高速化（GPU）とのやりとり。
//!
//! コンポのレイヤーを「ソースの画素・エフェクト・置き方（行列）・不透明度・描画モード」の列にして渡し、
//! GPU 側（`znd-gpu`）が 1 枚のキャンバスに下から順に重ねて、最後に 1 回だけ読み戻す。
//! レイヤーごとの往復が無くなるので、小さいレイヤーが大量にある条件でも速くなる。
//!
//! GPU に載せられないレイヤー（マスク・トラックマット・モーションブラー・シェイプ／テキスト・GPU 非対応の
//! エフェクトなど）は、CPU で出力空間のタイルまで作って渡し、重ねる処理だけを GPU で行う。
//! 結果は CPU の合成（`blend.rs`・`raster.rs`）と同じでなければならない。CPU が「正解」で、
//! `znd-gpu/tests/composite_parity.rs` がその差を検証する。

use crate::effects::{AccelEnv, AccelOp};
use crate::frame::{FrameBuf, Tile};
use std::sync::Arc;
use znd_core::effect::EvalParams;
use znd_core::{Affine, BlendMode};

/// レイヤーの絵の出どころ。
pub enum CompositeSource {
    /// 単色の矩形（ソース単位で `width`×`height`）。
    Solid { width: f64, height: f64, color: [f32; 4] },
    /// ソースの画素（乗算済みアルファ）。`ops` を GPU で適用してから置く。
    Image { buf: Arc<FrameBuf>, ops: Vec<(String, EvalParams)>, effect_scale: f32 },
    /// `width`×`height` 画素を `color` で塗った画像（エフェクト付きの平面）。GPU の上で作るので転送が要らない。
    Fill { width: usize, height: usize, color: [f32; 4], ops: Vec<(String, EvalParams)>, effect_scale: f32 },
    /// CPU で出力空間まで作ったタイル（不透明度・マスク・マット適用済み）。そのまま重ねる。
    Tile(Tile),
}

/// 重ねるレイヤー 1 枚。
pub struct CompositeLayer {
    pub source: CompositeSource,
    /// ソース画素座標 → 出力画素座標（`Tile` では使わない）。
    pub matrix: Affine,
    pub opacity: f32,
    pub blend: BlendMode,
}

/// 合成の依頼。
pub struct CompositeJob<'a> {
    pub width: usize,
    pub height: usize,
    /// リニアで合成する（false なら表示値で合成する）。
    pub linear: bool,
    /// 8/16bpc: レイヤーを重ねるたびに 0..1 に収める。
    pub clamp: bool,
    pub time_seconds: f64,
    pub cx: &'a crate::ColorXf,
    /// 下から順。
    pub layers: Vec<CompositeLayer>,
}

impl CompositeJob<'_> {
    pub fn env(&self, scale: f32) -> AccelEnv<'_> {
        AccelEnv { scale, time_seconds: self.time_seconds, cx: self.cx }
    }
}

/// 合成の高速化バックエンド（`znd-gpu` が実装する）。
pub trait CompositeAccel: Sync {
    /// このエフェクトを合成の途中で（画像の大きさに関係なく）処理できるか。
    fn supports_in_composite(&self, op: &AccelOp, env: &AccelEnv) -> bool;
    /// 出力の大きさを扱えるか。
    fn fits(&self, width: usize, height: usize) -> bool;
    /// 重ねた結果（乗算済みアルファ・リニア）。失敗したら `Err`（呼び出し側は CPU で描き直す）。
    fn composite(&self, job: &CompositeJob) -> Result<FrameBuf, String>;
}
