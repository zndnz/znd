//! ラウドネス計測（ITU-R BS.1770 / EBU R128）、トゥルーピーク、クリッピング検出。48kHz ステレオ前提。

use crate::SAMPLE_RATE;

#[derive(Debug, Clone, PartialEq)]
pub struct LoudnessReport {
    /// 統合ラウドネス（LUFS）。無音などで計測できないときは `None`。
    pub integrated_lufs: Option<f64>,
    /// トゥルーピーク（dBTP）。
    pub true_peak_db: f64,
    /// サンプルピーク（dBFS）。
    pub sample_peak_db: f64,
    /// フルスケールに達したサンプル数。
    pub clipped_samples: usize,
    /// クリッピングが起きている区間（秒）。近接するものはまとめる。
    pub clip_ranges: Vec<(f64, f64)>,
}

impl LoudnessReport {
    /// 上映提出向けの目安に照らした警告文。
    pub fn warnings(&self, target_lufs: f64, max_true_peak_db: f64) -> Vec<String> {
        let mut out = Vec::new();
        if self.clipped_samples > 0 {
            out.push(format!(
                "クリッピング: {} サンプルがフルスケールに達しています（{} 区間）",
                self.clipped_samples,
                self.clip_ranges.len()
            ));
        }
        if self.true_peak_db > max_true_peak_db {
            out.push(format!("トゥルーピークが {:.1} dBTP です（目安 {:.1} dBTP 以下）", self.true_peak_db, max_true_peak_db));
        }
        if let Some(l) = self.integrated_lufs {
            if l > target_lufs + 2.0 {
                out.push(format!(
                    "ラウドネスが {:.1} LUFS で大きすぎます（目安 {:.0} LUFS）。大音量の会場では歪みや聴き疲れの原因になります",
                    l, target_lufs
                ));
            } else if l < target_lufs - 6.0 {
                out.push(format!("ラウドネスが {:.1} LUFS で小さめです（目安 {:.0} LUFS）", l, target_lufs));
            }
        }
        out
    }
}

struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// K 特性フィルター（48kHz 用の係数。BS.1770 の表の値）。
fn k_weighting() -> [Biquad; 2] {
    [
        // 高域を持ち上げるシェルビング（頭部の音響効果）
        Biquad { b: [1.53512485958697, -2.69169618940638, 1.19839281085285], a: [-1.69065929318241, 0.73248077421585], z: [0.0; 2] },
        // 低域を落とすハイパス（RLB）
        Biquad { b: [1.0, -2.0, 1.0], a: [-1.99004745483398, 0.99007225036621], z: [0.0; 2] },
    ]
}

/// 統合ラウドネス。400ms ブロック（75% 重複）、絶対ゲート -70 LUFS、相対ゲート -10 LU。
pub fn integrated_lufs(stereo: &[f32]) -> Option<f64> {
    let frames = stereo.len() / 2;
    let block = (0.4 * SAMPLE_RATE as f64) as usize;
    let step = block / 4;
    if frames < block {
        return None;
    }
    // チャンネルごとに K 特性を掛けた二乗値の累積和
    let mut cum = vec![0.0f64; frames + 1];
    for ch in 0..2 {
        let mut f = k_weighting();
        for i in 0..frames {
            let shelved = f[0].run(stereo[i * 2 + ch] as f64);
            let y = f[1].run(shelved);
            cum[i + 1] += y * y;
        }
    }
    for i in 1..=frames {
        cum[i] += cum[i - 1];
    }
    // 上の 2 行で「各サンプルの L²+R²」→ その累積和になっている
    let powers: Vec<f64> = (0..=(frames - block) / step).map(|k| (cum[k * step + block] - cum[k * step]) / block as f64).collect();
    let lufs = |p: f64| -0.691 + 10.0 * p.max(1e-12).log10();
    let abs_gated: Vec<f64> = powers.iter().copied().filter(|p| lufs(*p) > -70.0).collect();
    if abs_gated.is_empty() {
        return None;
    }
    let mean = abs_gated.iter().sum::<f64>() / abs_gated.len() as f64;
    let rel = lufs(mean) - 10.0;
    let rel_gated: Vec<f64> = abs_gated.into_iter().filter(|p| lufs(*p) > rel).collect();
    if rel_gated.is_empty() {
        return None;
    }
    Some(lufs(rel_gated.iter().sum::<f64>() / rel_gated.len() as f64))
}

const OVERSAMPLE: usize = 4;
const TAPS: usize = 12;

/// 4 倍オーバーサンプリング用の、位相ごとの窓付き sinc 係数。
fn polyphase() -> Vec<[f32; TAPS]> {
    (0..OVERSAMPLE)
        .map(|phase| {
            let mut taps = [0.0f32; TAPS];
            let frac = phase as f64 / OVERSAMPLE as f64;
            let mut sum = 0.0;
            for (k, t) in taps.iter_mut().enumerate() {
                let x = k as f64 - (TAPS / 2 - 1) as f64 - frac;
                let sinc = if x.abs() < 1e-9 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
                let w = 0.5 + 0.5 * (std::f64::consts::PI * x / (TAPS / 2) as f64).cos();
                *t = (sinc * w.max(0.0)) as f32;
                sum += *t as f64;
            }
            taps.iter_mut().for_each(|t| *t /= sum as f32);
            taps
        })
        .collect()
}

/// トゥルーピーク（サンプル間のピークを含む最大絶対値、リニア）。
pub fn true_peak(stereo: &[f32]) -> f32 {
    let frames = stereo.len() / 2;
    let phases = polyphase();
    let mut peak = 0.0f32;
    for ch in 0..2 {
        for i in 0..frames {
            for taps in &phases {
                let mut acc = 0.0f32;
                for (k, t) in taps.iter().enumerate() {
                    let j = i as isize + k as isize - (TAPS / 2 - 1) as isize;
                    if j >= 0 && (j as usize) < frames {
                        acc += stereo[j as usize * 2 + ch] * t;
                    }
                }
                peak = peak.max(acc.abs());
            }
        }
    }
    peak
}

fn db(v: f64) -> f64 {
    20.0 * v.max(1e-12).log10()
}

pub fn analyze(stereo: &[f32]) -> LoudnessReport {
    let sample_peak = stereo.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let mut clipped = 0usize;
    let mut ranges: Vec<(f64, f64)> = Vec::new();
    for (i, frame) in stereo.chunks_exact(2).enumerate() {
        if frame[0].abs() >= 0.999 || frame[1].abs() >= 0.999 {
            clipped += 1;
            let t = i as f64 / SAMPLE_RATE as f64;
            match ranges.last_mut() {
                // 50ms 以内に続くクリップは 1 区間にまとめる
                Some(last) if t - last.1 < 0.05 => last.1 = t,
                _ => ranges.push((t, t)),
            }
        }
    }
    LoudnessReport {
        integrated_lufs: integrated_lufs(stereo),
        true_peak_db: db(true_peak(stereo) as f64),
        sample_peak_db: db(sample_peak as f64),
        clipped_samples: clipped,
        clip_ranges: ranges,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, seconds: f64, amp: f64, phase: f64) -> Vec<f32> {
        let n = (seconds * SAMPLE_RATE as f64) as usize;
        (0..n)
            .flat_map(|i| {
                let v = (amp * (2.0 * std::f64::consts::PI * freq * i as f64 / SAMPLE_RATE as f64 + phase).sin()) as f32;
                [v, v]
            })
            .collect()
    }

    #[test]
    fn ebu_reference_tone_measures_minus_23_lufs() {
        // EBU Tech 3341: 1kHz・-23 dBFS のステレオ正弦波は -23.0 LUFS
        let amp = 10f64.powf(-23.0 / 20.0);
        let l = integrated_lufs(&sine(1000.0, 20.0, amp, 0.0)).unwrap();
        assert!((l + 23.0).abs() < 0.1, "{l}");
        // -33 dBFS なら -33 LUFS
        let l = integrated_lufs(&sine(1000.0, 20.0, 10f64.powf(-33.0 / 20.0), 0.0)).unwrap();
        assert!((l + 33.0).abs() < 0.1, "{l}");
    }

    #[test]
    fn gating_ignores_silence() {
        // 10 秒の音 + 10 秒の無音。ゲートが無ければ 3dB 下がるが、ゲートにより音の部分だけで測られる
        let amp = 10f64.powf(-23.0 / 20.0);
        let mut s = sine(1000.0, 10.0, amp, 0.0);
        s.extend(vec![0.0f32; 48000 * 2 * 10]);
        let l = integrated_lufs(&s).unwrap();
        assert!((l + 23.0).abs() < 0.2, "{l}");
        assert_eq!(integrated_lufs(&vec![0.0f32; 48000 * 2 * 5]), None);
        assert_eq!(integrated_lufs(&[0.0; 100]), None);
    }

    #[test]
    fn true_peak_sees_intersample_peaks() {
        // fs/4 の正弦波を位相 45° で標本化すると、標本値は ±0.707 だが真のピークは 1.0
        let s = sine(12000.0, 0.5, 1.0, std::f64::consts::FRAC_PI_4);
        let sample_peak = s.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((sample_peak - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3);
        let tp = db(true_peak(&s) as f64);
        assert!(tp.abs() < 0.2, "{tp} dBTP");
        let r = analyze(&s);
        assert!((r.sample_peak_db + 3.01).abs() < 0.05 && r.clipped_samples == 0);
    }

    #[test]
    fn clipping_is_detected_with_ranges() {
        let mut s = sine(440.0, 1.0, 0.5, 0.0);
        // 0.25 秒付近と 0.75 秒付近で振り切れる
        for t in [0.25, 0.75] {
            let i = (t * 48000.0) as usize;
            for k in 0..100 {
                s[(i + k) * 2] = 1.0;
            }
        }
        let r = analyze(&s);
        assert_eq!(r.clipped_samples, 200);
        assert_eq!(r.clip_ranges.len(), 2);
        assert!((r.clip_ranges[0].0 - 0.25).abs() < 1e-3 && (r.clip_ranges[1].0 - 0.75).abs() < 1e-3);
        let w = r.warnings(-14.0, -1.0);
        assert!(w.iter().any(|m| m.contains("クリッピング")) && w.iter().any(|m| m.contains("トゥルーピーク")));
        assert!(analyze(&sine(1000.0, 5.0, 10f64.powf(-14.0 / 20.0), 0.0)).warnings(-14.0, -1.0).is_empty());
    }
}
