//! 時刻とフレームレート。
//!
//! 時刻は flicks（1/705,600,000 秒）の整数で持つ。この単位は 23.976〜60fps の
//! 全プリセットと 44.1k/48k/96kHz のサンプル周期を割り切るため、フレーム境界と
//! オーディオサンプル境界の両方を誤差なしで表現できる（音ハメの前提）。

use serde::{Deserialize, Serialize};
use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};

pub const FLICKS_PER_SECOND: i64 = 705_600_000;

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Time(pub i64);

impl Time {
    pub const ZERO: Time = Time(0);

    pub fn from_seconds(s: f64) -> Time {
        Time((s * FLICKS_PER_SECOND as f64).round() as i64)
    }

    pub fn seconds(self) -> f64 {
        self.0 as f64 / FLICKS_PER_SECOND as f64
    }
}

impl Add for Time {
    type Output = Time;
    fn add(self, o: Time) -> Time {
        Time(self.0 + o.0)
    }
}
impl Sub for Time {
    type Output = Time;
    fn sub(self, o: Time) -> Time {
        Time(self.0 - o.0)
    }
}
impl Neg for Time {
    type Output = Time;
    fn neg(self) -> Time {
        Time(-self.0)
    }
}
impl AddAssign for Time {
    fn add_assign(&mut self, o: Time) {
        self.0 += o.0;
    }
}
impl SubAssign for Time {
    fn sub_assign(&mut self, o: Time) {
        self.0 -= o.0;
    }
}

/// 有理数のフレームレート（例: 23.976fps = 24000/1001）。
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FrameRate {
    pub num: u32,
    pub den: u32,
}

impl FrameRate {
    pub const FPS_23_976: FrameRate = FrameRate { num: 24000, den: 1001 };
    pub const FPS_24: FrameRate = FrameRate { num: 24, den: 1 };
    pub const FPS_25: FrameRate = FrameRate { num: 25, den: 1 };
    pub const FPS_29_97: FrameRate = FrameRate { num: 30000, den: 1001 };
    pub const FPS_30: FrameRate = FrameRate { num: 30, den: 1 };
    pub const FPS_50: FrameRate = FrameRate { num: 50, den: 1 };
    pub const FPS_59_94: FrameRate = FrameRate { num: 60000, den: 1001 };
    pub const FPS_60: FrameRate = FrameRate { num: 60, den: 1 };

    pub const PRESETS: [FrameRate; 8] =
        [Self::FPS_23_976, Self::FPS_24, Self::FPS_25, Self::FPS_29_97, Self::FPS_30, Self::FPS_50, Self::FPS_59_94, Self::FPS_60];

    pub fn fps(self) -> f64 {
        self.num as f64 / self.den as f64
    }

    /// フレーム番号 → そのフレームの開始時刻。累積誤差が出ないよう毎回 0 起点で計算する。
    pub fn frame_to_time(self, frame: i64) -> Time {
        let n = frame as i128 * FLICKS_PER_SECOND as i128 * self.den as i128;
        Time(n.div_euclid(self.num as i128) as i64)
    }

    /// 時刻 → その時刻を含むフレーム番号（切り捨て）。
    pub fn time_to_frame(self, t: Time) -> i64 {
        let n = t.0 as i128 * self.num as i128;
        n.div_euclid(FLICKS_PER_SECOND as i128 * self.den as i128) as i64
    }

    /// 尺に含まれるフレーム数（端数は切り上げ）。
    pub fn frame_count(self, duration: Time) -> i64 {
        let f = self.time_to_frame(duration);
        if self.frame_to_time(f) < duration {
            f + 1
        } else {
            f
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_time_roundtrip_is_exact_for_all_presets() {
        for fr in FrameRate::PRESETS {
            for f in [0i64, 1, 2, 1000, 1001, 123_456, 10_000_000] {
                assert_eq!(fr.time_to_frame(fr.frame_to_time(f)), f, "{fr:?} frame {f}");
            }
            // 1 フレーム手前の最後の tick は前のフレームに属する
            let t = fr.frame_to_time(100);
            assert_eq!(fr.time_to_frame(Time(t.0 - 1)), 99);
        }
    }

    #[test]
    fn presets_divide_flicks_exactly() {
        for fr in FrameRate::PRESETS {
            assert_eq!((FLICKS_PER_SECOND as i128 * fr.den as i128) % fr.num as i128, 0);
        }
        for sr in [44_100, 48_000, 96_000] {
            assert_eq!(FLICKS_PER_SECOND % sr, 0);
        }
    }

    #[test]
    fn frame_count() {
        assert_eq!(FrameRate::FPS_30.frame_count(Time::from_seconds(5.0)), 150);
        assert_eq!(FrameRate::FPS_23_976.frame_count(FrameRate::FPS_23_976.frame_to_time(48)), 48);
        assert_eq!(FrameRate::FPS_30.frame_count(Time(1)), 1);
    }
}
