//! レイヤーマスク。パス → 被覆率 → 拡張・ぼかし → モードで合成 → レイヤーのアルファに掛ける。

use crate::blur::{gaussian, Edge};
use crate::frame::FrameBuf;
use crate::path::{fill_coverage, flatten};
use znd_core::shape::{FillRule, Mask, MaskMode};
use znd_core::{Affine, Time};

/// 可変幅フェザーで用意するぼかし段数。
const FEATHER_LEVELS: usize = 5;

/// マスク 1 枚の被覆率（バッファ全体、0..1）。`m` はレイヤー座標 → バッファ画素。
fn mask_alpha(mask: &Mask, t: Time, m: &Affine, w: usize, h: usize) -> Vec<f32> {
    let scale = (m.a * m.d - m.b * m.c).abs().sqrt();
    let tol = 0.05 / scale.max(1e-6);
    let mut line = flatten(&mask.path.eval(t), tol);
    let expansion = mask.expansion.eval(t);
    if expansion != 0.0 {
        line = line.offset(expansion);
    }
    let vertices: Vec<[f64; 2]> = mask
        .path
        .eval(t)
        .vertices
        .iter()
        .map(|v| {
            let (x, y) = m.apply(v.p[0], v.p[1]);
            [x, y]
        })
        .collect();
    let mut alpha = fill_coverage(&[line.transformed(m)], FillRule::NonZero, w, h).to_dense(w, h);

    // フェザー: ぼかし幅 F は「エッジを中心に F px かけて遷移する」ので σ ≈ F/2
    let f = mask.feather.eval(t);
    let (sx, sy) = ((f[0] * scale / 2.0) as f32, (f[1] * scale / 2.0) as f32);
    let variable = mask.vertex_feather.len() == vertices.len() && mask.vertex_feather.iter().any(|v| *v > 0.0);
    if !variable {
        if sx > 0.0 || sy > 0.0 {
            gaussian(&mut alpha, w, h, 1, sx, sy, Edge::Zero);
        }
        return alpha;
    }

    // 可変幅フェザー: 頂点ごとの追加ぼかしを逆距離加重で画素へ広げ、
    // 段階的にぼかした複数枚の間を画素ごとに補間する。
    let max_extra = mask.vertex_feather.iter().fold(0.0f64, |a, b| a.max(*b)) * scale / 2.0;
    let levels: Vec<Vec<f32>> = (0..FEATHER_LEVELS)
        .map(|k| {
            let extra = (max_extra * k as f64 / (FEATHER_LEVELS - 1) as f64) as f32;
            let mut a = alpha.clone();
            gaussian(&mut a, w, h, 1, sx + extra, sy + extra, Edge::Zero);
            a
        })
        .collect();
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let (mut num, mut den) = (0.0f64, 0.0f64);
            for (v, fz) in vertices.iter().zip(&mask.vertex_feather) {
                let d2 = (v[0] - x as f64 - 0.5).powi(2) + (v[1] - y as f64 - 0.5).powi(2);
                let wgt = 1.0 / (d2 + 1.0);
                num += wgt * fz;
                den += wgt;
            }
            let local = (num / den) * scale / 2.0;
            let pos = if max_extra > 0.0 { (local / max_extra).clamp(0.0, 1.0) * (FEATHER_LEVELS - 1) as f64 } else { 0.0 };
            let k = (pos.floor() as usize).min(FEATHER_LEVELS - 2);
            let frac = (pos - k as f64) as f32;
            let i = y * w + x;
            out[i] = levels[k][i] + (levels[k + 1][i] - levels[k][i]) * frac;
        }
    }
    out
}

/// レイヤーの全マスクを合成したアルファ（バッファ全体）。有効なマスクが無ければ `None`。
pub fn combined_alpha(masks: &[Mask], t: Time, m: &Affine, w: usize, h: usize) -> Option<Vec<f32>> {
    let active: Vec<&Mask> = masks.iter().filter(|k| k.mode != MaskMode::None).collect();
    let first = active.first()?;
    // 最初のマスクが加算系なら透明から、それ以外は不透明から始める
    let start = if matches!(first.mode, MaskMode::Add | MaskMode::Lighten) { 0.0 } else { 1.0 };
    let mut acc = vec![start; w * h];
    for mask in active {
        let mut a = mask_alpha(mask, t, m, w, h);
        let opacity = (mask.opacity.eval(t) / 100.0).clamp(0.0, 1.0) as f32;
        for (dst, v) in acc.iter_mut().zip(a.iter_mut()) {
            let v = (if mask.inverted { 1.0 - *v } else { *v }) * opacity;
            *dst = match mask.mode {
                MaskMode::None => *dst,
                MaskMode::Add => *dst + v - *dst * v,
                MaskMode::Subtract => *dst * (1.0 - v),
                MaskMode::Intersect => *dst * v,
                MaskMode::Lighten => dst.max(v),
                MaskMode::Darken => dst.min(v),
                MaskMode::Difference => (*dst - v).abs(),
            };
        }
    }
    Some(acc)
}

/// マスクをバッファに適用する（全チャンネルにアルファを掛ける）。
pub fn apply_masks(buf: &mut FrameBuf, masks: &[Mask], t: Time, m: &Affine) {
    if let Some(alpha) = combined_alpha(masks, t, m, buf.width, buf.height) {
        for (px, a) in buf.data.chunks_exact_mut(4).zip(&alpha) {
            px.iter_mut().for_each(|v| *v *= a);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use znd_core::shape::BezPath;
    use znd_core::Property;

    fn white(w: usize, h: usize) -> FrameBuf {
        let mut b = FrameBuf::new(w, h);
        b.data.fill(1.0);
        b
    }

    fn area(b: &FrameBuf) -> f64 {
        b.data.chunks_exact(4).map(|p| p[3] as f64).sum()
    }

    fn rect(x: f64, y: f64, w: f64, h: f64, mode: MaskMode) -> Mask {
        Mask { mode, ..Mask::new(BezPath::rect(x, y, w, h)) }
    }

    #[test]
    fn modes_combine_as_expected() {
        let run = |masks: &[Mask]| {
            let mut b = white(40, 40);
            apply_masks(&mut b, masks, Time::ZERO, &Affine::IDENTITY);
            area(&b)
        };
        let a = rect(0.0, 0.0, 20.0, 20.0, MaskMode::Add);
        assert!((run(std::slice::from_ref(&a)) - 400.0).abs() < 1e-3);
        assert!((run(&[a.clone(), rect(10.0, 0.0, 20.0, 20.0, MaskMode::Add)]) - 600.0).abs() < 1e-3);
        assert!((run(&[a.clone(), rect(10.0, 0.0, 20.0, 20.0, MaskMode::Subtract)]) - 200.0).abs() < 1e-3);
        assert!((run(&[a.clone(), rect(10.0, 0.0, 20.0, 20.0, MaskMode::Intersect)]) - 200.0).abs() < 1e-3);
        assert!((run(&[a.clone(), rect(10.0, 0.0, 20.0, 20.0, MaskMode::Difference)]) - 400.0).abs() < 1e-3);
        // 最初が減算なら「全体から引く」
        assert!((run(&[rect(0.0, 0.0, 20.0, 20.0, MaskMode::Subtract)]) - 1200.0).abs() < 1e-3);
        let inverted = Mask { inverted: true, ..a.clone() };
        assert!((run(&[inverted]) - 1200.0).abs() < 1e-3);
        let half = Mask { opacity: Property::constant(50.0), ..a.clone() };
        assert!((run(&[half]) - 200.0).abs() < 1e-3);
        // モード「なし」は無視され、マスクが無いのと同じ
        assert!((run(&[rect(0.0, 0.0, 5.0, 5.0, MaskMode::None)]) - 1600.0).abs() < 1e-3);
    }

    #[test]
    fn expansion_and_feather() {
        let mut m = rect(10.0, 10.0, 20.0, 20.0, MaskMode::Add);
        m.expansion = Property::constant(2.0);
        let mut b = white(40, 40);
        apply_masks(&mut b, &[m.clone()], Time::ZERO, &Affine::IDENTITY);
        assert!((area(&b) - 576.0).abs() < 1e-2);

        m.expansion = Property::constant(0.0);
        m.feather = Property::constant([8.0, 8.0]);
        let mut b = white(40, 40);
        apply_masks(&mut b, &[m.clone()], Time::ZERO, &Affine::IDENTITY);
        // ぼかしても総量は保たれ、エッジ上はおよそ 50%、中心はほぼ不透明
        assert!((area(&b) - 400.0).abs() < 4.0, "{}", area(&b));
        let edge = b.pixel(10, 20)[3];
        assert!((edge - 0.5).abs() < 0.08, "{edge}");
        assert!(b.pixel(20, 20)[3] > 0.9);

        // マトリクスで 2 倍に拡大すると、面積は 4 倍
        m.feather = Property::constant([0.0, 0.0]);
        let mut b = white(80, 80);
        apply_masks(&mut b, &[m], Time::ZERO, &Affine::scale(2.0, 2.0));
        assert!((area(&b) - 1600.0).abs() < 1e-2);
    }

    #[test]
    fn variable_feather_is_softer_near_the_feathered_vertex() {
        let mut m = rect(20.0, 20.0, 40.0, 40.0, MaskMode::Add);
        // 右下の頂点だけ大きくぼかす
        m.vertex_feather = vec![0.0, 0.0, 30.0, 0.0];
        let mut b = white(80, 80);
        apply_masks(&mut b, &[m], Time::ZERO, &Affine::IDENTITY);
        let sharp = b.pixel(22, 22)[3];
        let soft = b.pixel(57, 57)[3];
        assert!(sharp > 0.97, "{sharp}");
        assert!(soft < 0.8, "{soft}");
    }
}
