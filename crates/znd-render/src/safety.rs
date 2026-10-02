//! 光過敏性発作のリスク検出（明滅・赤色フラッシュ）。
//!
//! ITU-R BT.1702 と、いわゆるハーディングテストの考え方に沿う:
//! - 「フラッシュ」は、明るさが 20cd/m²（最大輝度 200cd/m² の画面で 10%）以上変化する、逆向きの遷移 1 対。
//!   暗い側の輝度が 160cd/m²（80%）未満のときだけ数える。
//! - 画面の 25% 以上の面積で同時に起きたものを対象にする。
//! - 任意の 1 秒間に 3 回を超えるフラッシュ（＝ 7 回以上の遷移）があれば危険。
//! - 飽和した赤への／からの遷移は、輝度変化が小さくても同じ基準で数える。
//!
//! これは制作中の目安であり、正式な適合検査（認証された検査装置）の代わりにはならない。

use crate::frame::FrameBuf;

const GRID_W: usize = 32;
const GRID_H: usize = 18;
/// 相対輝度（0..1）での遷移のしきい値。20cd/m² ÷ 200cd/m²。
const LUMA_STEP: f32 = 0.1;
const DARK_LIMIT: f32 = 0.8;
/// 飽和赤の指標 (R−G−B)×320 が 20 以上変化したら赤の遷移。
const RED_STEP: f32 = 20.0 / 320.0;
const AREA_LIMIT: f64 = 0.25;
/// 1 秒間に許される遷移の数（3 フラッシュ = 6 遷移）。
const MAX_TRANSITIONS: usize = 6;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum FlashKind {
    Luminance,
    Red,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FlashWarning {
    pub kind: FlashKind,
    /// 該当区間 [開始フレーム, 終了フレーム]。
    pub start_frame: i64,
    pub end_frame: i64,
    /// 区間内で最も多かった 1 秒あたりのフラッシュ数。
    pub flashes_per_second: f64,
}

impl FlashWarning {
    pub fn message(&self) -> String {
        let what = match self.kind {
            FlashKind::Luminance => "明滅",
            FlashKind::Red => "赤色フラッシュ",
        };
        format!("{what}が 1 秒あたり約 {:.1} 回あります（3 回を超えると光過敏性発作の危険があります）", self.flashes_per_second)
    }
}

/// 1 フレームを粗い格子に縮めた測定値。
#[derive(Debug, Clone)]
pub struct FrameStats {
    /// 相対輝度（0..1、リニア）。
    luma: Vec<f32>,
    /// 飽和赤の指標。飽和した赤でなければ 0。
    red: Vec<f32>,
}

/// フレームから測定値を取る。作業空間リニアの画像（背景合成済み）を渡す。
pub fn frame_stats(buf: &FrameBuf) -> FrameStats {
    let mut luma = vec![0.0f32; GRID_W * GRID_H];
    let mut red = vec![0.0f32; GRID_W * GRID_H];
    let mut count = vec![0.0f32; GRID_W * GRID_H];
    for y in 0..buf.height {
        let gy = y * GRID_H / buf.height;
        for x in 0..buf.width {
            let gx = x * GRID_W / buf.width;
            let p = buf.pixel(x, y);
            let (r, g, b) = (p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0), p[2].clamp(0.0, 1.0));
            let i = gy * GRID_W + gx;
            luma[i] += 0.2126 * r + 0.7152 * g + 0.0722 * b;
            // 飽和した赤: R/(R+G+B) >= 0.8
            if r + g + b > 1e-4 && r / (r + g + b) >= 0.8 {
                red[i] += (r - g - b).max(0.0);
            }
            count[i] += 1.0;
        }
    }
    for i in 0..luma.len() {
        let n = count[i].max(1.0);
        luma[i] /= n;
        red[i] /= n;
    }
    FrameStats { luma, red }
}

/// 格子の各マスで「直前の極値から step 以上、逆向きに動いたか」を追い、
/// 画面の 25% 以上で同時に遷移が起きたフレーム番号を返す。
fn transitions(values: &[&Vec<f32>], step: f32, dark_limit: Option<f32>) -> Vec<usize> {
    let cells = GRID_W * GRID_H;
    let Some(first) = values.first() else { return Vec::new() };
    // マスごとの (直前の極値, 最後に遷移した向き)
    let mut extreme: Vec<f32> = first.to_vec();
    let mut dir = vec![0i8; cells];
    let mut out = Vec::new();
    for (f, v) in values.iter().enumerate().skip(1) {
        let (mut up, mut down) = (0usize, 0usize);
        for c in 0..cells {
            let d = v[c] - extreme[c];
            let darker = v[c].min(extreme[c]);
            let ok = dark_limit.is_none_or(|l| darker < l);
            if d >= step && dir[c] <= 0 && ok {
                up += 1;
                dir[c] = 1;
                extreme[c] = v[c];
            } else if d <= -step && dir[c] >= 0 && ok {
                down += 1;
                dir[c] = -1;
                extreme[c] = v[c];
            } else if (dir[c] > 0 && v[c] > extreme[c]) || (dir[c] < 0 && v[c] < extreme[c]) {
                // 同じ向きに動き続けている間は極値を更新する
                extreme[c] = v[c];
            } else if dir[c] == 0 {
                // まだ一度も遷移していないマスは、振れ幅の小さい側の値を基準に取り直す
                if d.abs() < step {
                    continue;
                }
            }
        }
        if up.max(down) as f64 / cells as f64 >= AREA_LIMIT {
            out.push(f);
        }
    }
    out
}

fn windows(transitions: &[usize], fps: f64, kind: FlashKind) -> Vec<FlashWarning> {
    let per_second = fps.round().max(1.0) as usize;
    let mut warnings: Vec<FlashWarning> = Vec::new();
    for (i, start) in transitions.iter().enumerate() {
        // この遷移から 1 秒以内にある遷移の数
        let n = transitions[i..].iter().take_while(|t| **t < start + per_second).count();
        if n > MAX_TRANSITIONS {
            let end = transitions[i + n - 1];
            let rate = n as f64 / 2.0;
            match warnings.last_mut() {
                Some(w) if w.end_frame >= *start as i64 => {
                    w.end_frame = w.end_frame.max(end as i64);
                    w.flashes_per_second = w.flashes_per_second.max(rate);
                }
                _ => warnings.push(FlashWarning { kind, start_frame: *start as i64, end_frame: end as i64, flashes_per_second: rate }),
            }
        }
    }
    warnings
}

/// フレーム列（先頭がフレーム 0）を解析し、危険な区間を返す。
pub fn analyze(stats: &[FrameStats], fps: f64) -> Vec<FlashWarning> {
    let luma: Vec<&Vec<f32>> = stats.iter().map(|s| &s.luma).collect();
    let red: Vec<&Vec<f32>> = stats.iter().map(|s| &s.red).collect();
    let mut out = windows(&transitions(&luma, LUMA_STEP, Some(DARK_LIMIT)), fps, FlashKind::Luminance);
    out.extend(windows(&transitions(&red, RED_STEP, None), fps, FlashKind::Red));
    out.sort_by_key(|w| w.start_frame);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(rgb: [f32; 3]) -> FrameBuf {
        let mut b = FrameBuf::new(64, 36);
        b.data.chunks_exact_mut(4).for_each(|p| p.copy_from_slice(&[rgb[0], rgb[1], rgb[2], 1.0]));
        b
    }

    /// 30fps で `hz` 回／秒の明滅（a と b を交互に）。`area` は画面のうち明滅する割合。
    fn flicker(hz: f64, seconds: f64, a: [f32; 3], b: [f32; 3], area: f64) -> Vec<FrameStats> {
        (0..(seconds * 30.0) as usize)
            .map(|f| {
                let phase = (f as f64 / 30.0 * hz * 2.0).floor() as i64;
                let mut buf = solid(a);
                if phase % 2 == 1 {
                    let w = (64.0 * area).round() as usize;
                    for y in 0..36 {
                        for x in 0..w {
                            buf.set_pixel(x, y, [b[0], b[1], b[2], 1.0]);
                        }
                    }
                }
                frame_stats(&buf)
            })
            .collect()
    }

    const BLACK: [f32; 3] = [0.0; 3];
    const WHITE: [f32; 3] = [1.0; 3];

    #[test]
    fn fast_full_screen_flashing_is_flagged_slow_is_not() {
        let w = analyze(&flicker(5.0, 3.0, BLACK, WHITE, 1.0), 30.0);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].kind, FlashKind::Luminance);
        assert!((w[0].flashes_per_second - 5.0).abs() <= 0.5, "{}", w[0].flashes_per_second);
        assert!(w[0].start_frame <= 4 && w[0].end_frame >= 80, "{w:?}");
        assert!(w[0].message().contains("明滅"));
        // 2 回／秒、3 回／秒（ちょうど上限）は安全
        assert!(analyze(&flicker(2.0, 3.0, BLACK, WHITE, 1.0), 30.0).is_empty());
        assert!(analyze(&flicker(3.0, 3.0, BLACK, WHITE, 1.0), 30.0).is_empty());
        // 4 回／秒は危険
        assert_eq!(analyze(&flicker(4.0, 3.0, BLACK, WHITE, 1.0), 30.0).len(), 1);
    }

    #[test]
    fn small_area_low_contrast_and_bright_scenes_are_safe() {
        // 画面の 15% だけが明滅
        assert!(analyze(&flicker(8.0, 2.0, BLACK, WHITE, 0.15), 30.0).is_empty());
        // 30% なら対象
        assert_eq!(analyze(&flicker(8.0, 2.0, BLACK, WHITE, 0.30), 30.0).len(), 1);
        // 輝度差 5% の明滅
        assert!(analyze(&flicker(8.0, 2.0, [0.30; 3], [0.35; 3], 1.0), 30.0).is_empty());
        // 暗い側も 80% 以上の明るい画面どうしの明滅は数えない
        assert!(analyze(&flicker(8.0, 2.0, [0.85; 3], [1.0; 3], 1.0), 30.0).is_empty());
        // ゆっくりしたフェード
        let fade: Vec<FrameStats> = (0..90).map(|f| frame_stats(&solid([f as f32 / 90.0; 3]))).collect();
        assert!(analyze(&fade, 30.0).is_empty());
        assert!(analyze(&[], 30.0).is_empty());
    }

    #[test]
    fn saturated_red_flashes_are_flagged_even_at_low_luminance_change() {
        // 暗い赤 ↔ 黒: 輝度差は 0.2126×0.4 = 8.5% でしきい値未満だが、赤色フラッシュとして検出する
        let w = analyze(&flicker(5.0, 2.0, BLACK, [0.4, 0.0, 0.0], 1.0), 30.0);
        assert_eq!(w.len(), 1, "{w:?}");
        assert_eq!(w[0].kind, FlashKind::Red);
        // 明るい赤 ↔ 黒は、輝度と赤の両方で検出される
        let kinds: Vec<FlashKind> = analyze(&flicker(5.0, 2.0, BLACK, [1.0, 0.0, 0.0], 1.0), 30.0).iter().map(|w| w.kind).collect();
        assert!(kinds.contains(&FlashKind::Red) && kinds.contains(&FlashKind::Luminance));
        // 彩度の低い赤（ピンク）は赤色フラッシュではない
        let pink = analyze(&flicker(5.0, 2.0, [0.5, 0.45, 0.45], [0.55, 0.45, 0.45], 1.0), 30.0);
        assert!(pink.is_empty(), "{pink:?}");
    }

    #[test]
    fn separate_bursts_give_separate_ranges() {
        let mut s = flicker(6.0, 1.5, BLACK, WHITE, 1.0);
        s.extend(flicker(0.0, 3.0, BLACK, WHITE, 1.0));
        s.extend(flicker(6.0, 1.5, BLACK, WHITE, 1.0));
        let w = analyze(&s, 30.0);
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(w[0].end_frame < 60 && w[1].start_frame > 120);
    }
}
