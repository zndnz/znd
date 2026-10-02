//! ビートグリッドとスナップ。

use crate::time::{FrameRate, Time, FLICKS_PER_SECOND};
use serde::{Deserialize, Serialize};

/// BPM・拍子・オフセットで決まる拍の格子。
#[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
pub struct BeatGrid {
    pub bpm: f64,
    /// 1 小節の拍数（4/4 なら 4）。
    #[serde(default = "four")]
    pub beats_per_bar: u32,
    /// 1 拍の分割数（スナップ先の細かさ。4 で 16 分音符）。
    #[serde(default = "one")]
    pub subdivision: u32,
    /// 最初の拍（小節頭）のコンポ時刻。
    #[serde(default)]
    pub offset: Time,
}

fn four() -> u32 {
    4
}
fn one() -> u32 {
    1
}

impl BeatGrid {
    pub fn new(bpm: f64) -> Self {
        BeatGrid { bpm, beats_per_bar: 4, subdivision: 1, offset: Time::ZERO }
    }

    fn beat_flicks(&self) -> f64 {
        60.0 / self.bpm * FLICKS_PER_SECOND as f64
    }

    /// `n` 拍目（0 始まり、小数可）の時刻。
    pub fn beat_time(&self, n: f64) -> Time {
        self.offset + Time((n * self.beat_flicks()).round() as i64)
    }

    /// 時刻 → 拍番号（小数）。
    pub fn beat_at(&self, t: Time) -> f64 {
        (t - self.offset).0 as f64 / self.beat_flicks()
    }

    /// 最寄りの格子点（分割込み）にスナップする。
    pub fn snap(&self, t: Time) -> Time {
        let sub = self.subdivision.max(1) as f64;
        self.beat_time((self.beat_at(t) * sub).round() / sub)
    }

    /// 格子点にスナップしたうえで、そのフレームレートのフレーム境界に丸める（最寄りフレーム）。
    pub fn snap_to_frame(&self, t: Time, fr: FrameRate) -> Time {
        nearest_frame(self.snap(t), fr)
    }

    /// `range` 内の格子点を (時刻, 小節頭か, 拍頭か) で列挙する。
    pub fn lines(&self, start: Time, end: Time) -> Vec<(Time, bool, bool)> {
        if self.bpm <= 0.0 || end <= start {
            return Vec::new();
        }
        let sub = self.subdivision.max(1) as i64;
        let first = (self.beat_at(start) * sub as f64).ceil() as i64;
        let last = (self.beat_at(end) * sub as f64).floor() as i64;
        (first..=last)
            .map(|k| {
                let on_beat = k.rem_euclid(sub) == 0;
                let on_bar = on_beat && (k.div_euclid(sub)).rem_euclid(self.beats_per_bar.max(1) as i64) == 0;
                (self.beat_time(k as f64 / sub as f64), on_bar, on_beat)
            })
            .collect()
    }
}

/// 最寄りのフレーム境界。
pub fn nearest_frame(t: Time, fr: FrameRate) -> Time {
    let f = fr.time_to_frame(t);
    let (a, b) = (fr.frame_to_time(f), fr.frame_to_time(f + 1));
    if t - a <= b - t {
        a
    } else {
        b
    }
}

/// タップ入力（時刻列）から BPM とオフセットを推定する。最小二乗で等間隔の格子を当てはめる。
pub fn grid_from_taps(taps: &[Time]) -> Option<BeatGrid> {
    if taps.len() < 2 {
        return None;
    }
    let n = taps.len() as f64;
    let xs: Vec<f64> = (0..taps.len()).map(|i| i as f64).collect();
    let ys: Vec<f64> = taps.iter().map(|t| t.seconds()).collect();
    let (mx, my) = (xs.iter().sum::<f64>() / n, ys.iter().sum::<f64>() / n);
    let sxy: f64 = xs.iter().zip(&ys).map(|(x, y)| (x - mx) * (y - my)).sum();
    let sxx: f64 = xs.iter().map(|x| (x - mx) * (x - mx)).sum();
    let interval = sxy / sxx;
    if interval <= 0.0 {
        return None;
    }
    let offset = my - interval * mx;
    Some(BeatGrid { bpm: 60.0 / interval, beats_per_bar: 4, subdivision: 1, offset: Time::from_seconds(offset) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_to_grid_and_frame() {
        let g = BeatGrid { bpm: 120.0, beats_per_bar: 4, subdivision: 2, offset: Time::from_seconds(0.1) };
        assert_eq!(g.snap(Time::from_seconds(0.62)), Time::from_seconds(0.6));
        assert_eq!(g.snap(Time::from_seconds(0.36)), Time::from_seconds(0.35));
        // 23.976fps・BPM 173 でも、スナップ結果は常に「格子点に最も近いフレーム境界」
        let fr = FrameRate::FPS_23_976;
        let g = BeatGrid::new(173.0);
        for beat in 0..500 {
            let exact = g.beat_time(beat as f64);
            let snapped = g.snap_to_frame(exact + Time(12345), fr);
            assert_eq!(fr.frame_to_time(fr.time_to_frame(snapped)), snapped, "フレーム境界でない");
            assert!((snapped - exact).0.abs() <= fr.frame_to_time(1).0 / 2, "最寄りフレームでない");
        }
    }

    #[test]
    fn grid_lines_mark_bars_and_beats() {
        let g = BeatGrid { bpm: 60.0, beats_per_bar: 3, subdivision: 2, offset: Time::ZERO };
        let lines = g.lines(Time::ZERO, Time::from_seconds(3.0));
        assert_eq!(lines.len(), 7);
        assert_eq!(lines[0], (Time::ZERO, true, true));
        assert_eq!(lines[1], (Time::from_seconds(0.5), false, false));
        assert_eq!(lines[2], (Time::from_seconds(1.0), false, true));
        assert_eq!(lines[6], (Time::from_seconds(3.0), true, true));
    }

    #[test]
    fn taps_recover_bpm() {
        let taps: Vec<Time> = (0..8).map(|i| Time::from_seconds(0.25 + i as f64 * 0.4 + if i % 2 == 0 { 0.004 } else { -0.004 })).collect();
        let g = grid_from_taps(&taps).unwrap();
        assert!((g.bpm - 150.0).abs() < 1.0, "bpm {}", g.bpm);
        assert!((g.offset.seconds() - 0.25).abs() < 0.01);
        assert!(grid_from_taps(&taps[..1]).is_none());
    }
}
