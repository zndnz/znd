//! 画質・差分の検査: 上映プレビューの警告オーバーレイ、版間のビジュアル差分、2D ポイントトラッカー。

use crate::color::OutputXf;
use crate::frame::FrameBuf;

/// 上映プレビュー用の検査結果。
#[derive(Debug, Clone, PartialEq)]
pub struct ScreeningReport {
    /// 白飛び（全チャンネルが最大に張り付いた）画素の割合。
    pub clipped_white: f64,
    /// 黒つぶれ（真っ黒ではないが、暗い会場では黒と区別できない）画素の割合。
    pub crushed_black: f64,
    /// バンディングの危険がある（ごく緩いグラデーションが広く続く）画素の割合。
    pub banding_risk: f64,
    /// 警告箇所を色で示した RGBA8（白飛び = 赤、黒つぶれ = 青、バンディング = 緑）。それ以外は透明。
    pub overlay: Vec<u8>,
}

/// 黒つぶれとみなす表示値の上限（8bit で 4 相当）。プロジェクターの黒浮きに埋もれる範囲。
const CRUSH_LIMIT: f32 = 4.0 / 255.0;
const WHITE_LIMIT: f32 = 254.5 / 255.0;

pub fn screening_report(buf: &FrameBuf, xf: &OutputXf) -> ScreeningReport {
    let (w, h) = (buf.width, buf.height);
    // 出力の符号化値（0..1）と、その輝度
    let enc: Vec<[f32; 3]> = buf
        .data
        .chunks_exact(4)
        .map(|p| {
            let a = if p[3] > 0.0 { p[3] } else { 1.0 };
            xf.encode([p[0] / a, p[1] / a, p[2] / a])
        })
        .collect();
    let luma: Vec<f32> = enc.iter().map(|c| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]).collect();
    let mut overlay = vec![0u8; w * h * 4];
    let (mut white, mut black, mut band) = (0usize, 0usize, 0usize);
    // 8bit のコード 1 つぶんの明るさの差が、何画素にわたって続くか。数画素以上なら縞として見える
    let risky = |x: usize, y: usize| -> bool {
        if x == 0 || y == 0 || x + 1 >= w || y + 1 >= h {
            return false;
        }
        let gx = (luma[y * w + x + 1] - luma[y * w + x - 1]) * 0.5 * 255.0;
        let gy = (luma[(y + 1) * w + x] - luma[(y - 1) * w + x]) * 0.5 * 255.0;
        let g = gx.hypot(gy);
        // 1 コードあたり 4〜200 画素の傾き（それより急なら縞は細かすぎて見えず、それより緩いなら平坦）
        g > 0.005 && g < 0.25
    };
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let c = enc[i];
            let o = &mut overlay[i * 4..i * 4 + 4];
            if c[0] >= WHITE_LIMIT && c[1] >= WHITE_LIMIT && c[2] >= WHITE_LIMIT {
                white += 1;
                o.copy_from_slice(&[255, 40, 40, 200]);
            } else if luma[i] > 0.0 && luma[i] < CRUSH_LIMIT {
                black += 1;
                o.copy_from_slice(&[60, 90, 255, 200]);
            } else if risky(x, y)
                && risky(x.saturating_sub(2), y)
                && risky((x + 2).min(w - 1), y)
                && risky(x, y.saturating_sub(2))
                && risky(x, (y + 2).min(h - 1))
            {
                band += 1;
                o.copy_from_slice(&[40, 220, 90, 110]);
            }
        }
    }
    let n = (w * h).max(1) as f64;
    ScreeningReport { clipped_white: white as f64 / n, crushed_black: black as f64 / n, banding_risk: band as f64 / n, overlay }
}

/// 2 つの版の絵の違い。
#[derive(Debug, Clone, PartialEq)]
pub struct DiffReport {
    /// 違いのある画素数。
    pub changed_pixels: usize,
    /// 違いのある範囲 [x0, y0, x1, y1)。違いが無ければ `None`。
    pub bbox: Option<[usize; 4]>,
    /// 最大の差（表示値、0..1）。
    pub max_difference: f32,
    /// 違いの大きさを赤の濃さで示した画像（もとの絵を暗くした上に重ねる）。
    pub heatmap: FrameBuf,
}

/// 2 枚のフレームを比べる。`threshold` は表示値での差のしきい値（例: 2/255）。
pub fn diff_frames(a: &FrameBuf, b: &FrameBuf, threshold: f32) -> Option<DiffReport> {
    if a.width != b.width || a.height != b.height {
        return None;
    }
    let xf = OutputXf::SRGB;
    let (w, h) = (a.width, a.height);
    let mut heatmap = FrameBuf::new(w, h);
    let (mut changed, mut max_diff) = (0usize, 0.0f32);
    let mut bbox: Option<[usize; 4]> = None;
    for y in 0..h {
        for x in 0..w {
            let (pa, pb) = (a.pixel(x, y), b.pixel(x, y));
            let (ea, eb) = (xf.encode([pa[0], pa[1], pa[2]]), xf.encode([pb[0], pb[1], pb[2]]));
            let d = (0..3).map(|k| (ea[k] - eb[k]).abs()).fold((pa[3] - pb[3]).abs(), f32::max);
            max_diff = max_diff.max(d);
            // 背景として新しい版を暗く敷く
            let base = [pb[0] * 0.25, pb[1] * 0.25, pb[2] * 0.25, 1.0];
            if d > threshold {
                changed += 1;
                bbox = Some(match bbox {
                    None => [x, y, x + 1, y + 1],
                    Some(r) => [r[0].min(x), r[1].min(y), r[2].max(x + 1), r[3].max(y + 1)],
                });
                let k = (d * 4.0).clamp(0.3, 1.0);
                heatmap.set_pixel(x, y, [base[0] + k, base[1] * (1.0 - k), base[2] * (1.0 - k), 1.0]);
            } else {
                heatmap.set_pixel(x, y, base);
            }
        }
    }
    Some(DiffReport { changed_pixels: changed, bbox, max_difference: max_diff, heatmap })
}

/// 輝度画像（トラッキング用）。
pub struct Gray {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl Gray {
    pub fn from_frame(buf: &FrameBuf) -> Gray {
        Gray {
            width: buf.width,
            height: buf.height,
            data: buf.data.chunks_exact(4).map(|p| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]).collect(),
        }
    }

    fn sample(&self, x: f64, y: f64) -> f32 {
        let (x, y) = (x.clamp(0.0, self.width as f64 - 1.0), y.clamp(0.0, self.height as f64 - 1.0));
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
        let at = |x: usize, y: usize| self.data[y * self.width + x];
        let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * fx;
        let bot = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * fx;
        top + (bot - top) * fy
    }
}

/// `reference` の `ref_point` 周辺の模様を、`next` の `guess` の近くから探す。
/// `pattern` は特徴の半径、`search` は探索半径（画素）。
/// 正規化相互相関が最大の位置（サブピクセル）と、その相関値（1 に近いほど確実）を返す。
pub fn track_step(reference: &Gray, ref_point: [f64; 2], next: &Gray, guess: [f64; 2], pattern: usize, search: usize) -> ([f64; 2], f32) {
    let p = pattern as i64;
    let n = ((2 * p + 1) * (2 * p + 1)) as f32;
    // テンプレート（平均を引いておく）
    let mut tmpl = Vec::with_capacity(n as usize);
    for dy in -p..=p {
        for dx in -p..=p {
            tmpl.push(reference.sample(ref_point[0] + dx as f64, ref_point[1] + dy as f64));
        }
    }
    let tmean = tmpl.iter().sum::<f32>() / n;
    let tnorm = tmpl.iter().map(|v| (v - tmean).powi(2)).sum::<f32>().sqrt().max(1e-9);
    let score = |cx: f64, cy: f64| -> f32 {
        let mut vals = Vec::with_capacity(tmpl.len());
        for dy in -p..=p {
            for dx in -p..=p {
                vals.push(next.sample(cx + dx as f64, cy + dy as f64));
            }
        }
        let mean = vals.iter().sum::<f32>() / n;
        let norm = vals.iter().map(|v| (v - mean).powi(2)).sum::<f32>().sqrt().max(1e-9);
        vals.iter().zip(&tmpl).map(|(v, t)| (v - mean) * (t - tmean)).sum::<f32>() / (norm * tnorm)
    };
    let s = search as i64;
    let mut best = (f32::MIN, 0i64, 0i64);
    for dy in -s..=s {
        for dx in -s..=s {
            let v = score(guess[0] + dx as f64, guess[1] + dy as f64);
            if v > best.0 {
                best = (v, dx, dy);
            }
        }
    }
    // 最大点の周りを放物線で補間してサブピクセル位置を求める
    let (bx, by) = (guess[0] + best.1 as f64, guess[1] + best.2 as f64);
    let refine = |m: f32, c: f32, pl: f32| -> f64 {
        let denom = m - 2.0 * c + pl;
        if denom.abs() < 1e-9 {
            0.0
        } else {
            (0.5 * (m - pl) / denom).clamp(-0.5, 0.5) as f64
        }
    };
    let ox = refine(score(bx - 1.0, by), best.0, score(bx + 1.0, by));
    let oy = refine(score(bx, by - 1.0), best.0, score(bx, by + 1.0));
    ([bx + ox, by + oy], best.0)
}

/// 連続するフレームにわたって 1 点を追跡する。各フレームでの位置を返す（先頭は開始位置）。
///
/// 模様の基準は最初のフレームから取り続けるので、誤差が積み上がらない（ドリフトしない）。
/// 見た目が変わって最初の模様と合わなくなったら、直前のフレームを基準に取り直す。
/// それでも相関が `min_confidence` を下回ったら、そこで追跡を打ち切る。
pub fn track_point(frames: &[Gray], start: [f64; 2], pattern: usize, search: usize, min_confidence: f32) -> Vec<[f64; 2]> {
    let mut out = vec![start];
    let Some(first) = frames.first() else { return out };
    let (mut reference, mut ref_point) = (first, start);
    for i in 1..frames.len() {
        let guess = *out.last().unwrap();
        let (mut pos, mut conf) = track_step(reference, ref_point, &frames[i], guess, pattern, search);
        if conf < min_confidence {
            (reference, ref_point) = (&frames[i - 1], guess);
            (pos, conf) = track_step(reference, ref_point, &frames[i], guess, pattern, search);
            if conf < min_confidence {
                break;
            }
        }
        out.push(pos);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::srgb_to_linear;

    fn flat(w: usize, h: usize, display: f32) -> FrameBuf {
        let v = srgb_to_linear(display);
        let mut b = FrameBuf::new(w, h);
        b.data.chunks_exact_mut(4).for_each(|p| p.copy_from_slice(&[v, v, v, 1.0]));
        b
    }

    #[test]
    fn screening_flags_white_clip_black_crush_and_banding() {
        let r = screening_report(&flat(32, 32, 0.5), &OutputXf::SRGB);
        assert_eq!((r.clipped_white, r.crushed_black, r.banding_risk), (0.0, 0.0, 0.0));
        assert!(r.overlay.iter().all(|v| *v == 0));
        assert_eq!(screening_report(&flat(32, 32, 1.0), &OutputXf::SRGB).clipped_white, 1.0);
        // 純黒は「意図した黒」なので警告しない。ごく暗いグレーは黒つぶれ
        assert_eq!(screening_report(&flat(32, 32, 0.0), &OutputXf::SRGB).crushed_black, 0.0);
        assert_eq!(screening_report(&flat(32, 32, 2.0 / 255.0), &OutputXf::SRGB).crushed_black, 1.0);
        // 600px かけて表示値が 0.30 → 0.36 に変わる（15 コードぶん）緩いグラデーション
        let mut g = FrameBuf::new(600, 40);
        for y in 0..40 {
            for x in 0..600 {
                let v = srgb_to_linear(0.30 + 0.06 * x as f32 / 600.0);
                g.set_pixel(x, y, [v, v, v, 1.0]);
            }
        }
        assert!(screening_report(&g, &OutputXf::SRGB).banding_risk > 0.7);
        // 急なグラデーション（100px で 0 → 1）は対象外
        let mut steep = FrameBuf::new(100, 40);
        for y in 0..40 {
            for x in 0..100 {
                let v = srgb_to_linear(0.1 + 0.8 * x as f32 / 100.0);
                steep.set_pixel(x, y, [v, v, v, 1.0]);
            }
        }
        assert_eq!(screening_report(&steep, &OutputXf::SRGB).banding_risk, 0.0);
    }

    #[test]
    fn diff_finds_changed_region() {
        let a = flat(64, 48, 0.4);
        let mut b = a.clone();
        assert_eq!(diff_frames(&a, &b, 2.0 / 255.0).unwrap().bbox, None);
        for y in 10..20 {
            for x in 30..45 {
                b.set_pixel(x, y, [1.0, 0.0, 0.0, 1.0]);
            }
        }
        let d = diff_frames(&a, &b, 2.0 / 255.0).unwrap();
        assert_eq!(d.changed_pixels, 150);
        assert_eq!(d.bbox, Some([30, 10, 45, 20]));
        assert!(d.max_difference > 0.5);
        assert!(d.heatmap.pixel(35, 15)[0] > d.heatmap.pixel(5, 5)[0]);
        assert!(diff_frames(&a, &flat(10, 10, 0.4), 0.01).is_none());
    }

    /// 滑らかな模様を (dx, dy) だけずらした画像。
    fn texture(dx: f64, dy: f64) -> Gray {
        let (w, h) = (96, 96);
        let data = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f64 - dx, (i / w) as f64 - dy);
                ((x * 0.31).sin() * (y * 0.23).cos()
                    + (x * 0.07 + y * 0.11).sin() * 0.5
                    + ((x - 48.0).powi(2) + (y - 48.0).powi(2)).sqrt().cos() * 0.3) as f32
            })
            .collect();
        Gray { width: w, height: h, data }
    }

    #[test]
    fn tracker_follows_subpixel_motion() {
        // フレームごとに (1.3, -0.7) 画素ずつ動く
        let frames: Vec<Gray> = (0..12).map(|i| texture(1.3 * i as f64, -0.7 * i as f64)).collect();
        let track = track_point(&frames, [48.0, 48.0], 8, 4, 0.5);
        assert_eq!(track.len(), 12);
        for (i, p) in track.iter().enumerate() {
            let expect = [48.0 + 1.3 * i as f64, 48.0 - 0.7 * i as f64];
            assert!((p[0] - expect[0]).abs() < 0.5 && (p[1] - expect[1]).abs() < 0.5, "frame {i}: {p:?} vs {expect:?}");
        }
        // 模様が無くなったら追跡を止める
        let mut lost = vec![texture(0.0, 0.0)];
        lost.push(Gray { width: 96, height: 96, data: vec![0.0; 96 * 96] });
        assert_eq!(track_point(&lost, [48.0, 48.0], 8, 4, 0.5).len(), 1);
    }
}
