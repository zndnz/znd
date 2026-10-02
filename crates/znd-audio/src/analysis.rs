//! 音声解析: 波形ピーク、スペクトログラム、オンセット検出、帯域別音量。入力はモノラル。

use crate::SAMPLE_RATE;
use rayon::prelude::*;
use znd_core::dsp::{fft, magnitude_spectrum, Complex};
use znd_core::{FrameRate, Keyframe, Property, Time};

/// 波形表示用のピーク。`bucket` サンプルごとの (最小, 最大)。
pub fn peaks(mono: &[f32], bucket: usize) -> Vec<(f32, f32)> {
    mono.chunks(bucket.max(1)).map(|c| c.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)))).collect()
}

/// スペクトログラム。列ごとに `n/2+1` 個の dB 値（0dB = フルスケール正弦波）。
pub struct Spectrogram {
    pub columns: Vec<Vec<f32>>,
    pub hop: usize,
    pub bins: usize,
}

pub fn spectrogram(mono: &[f32], n: usize, hop: usize) -> Spectrogram {
    let count = if mono.len() >= n { (mono.len() - n) / hop + 1 } else { 0 };
    let columns = (0..count)
        .into_par_iter()
        .map(|k| magnitude_spectrum(&mono[k * hop..k * hop + n]).into_iter().map(|m| 20.0 * m.max(1e-6).log10()).collect())
        .collect();
    Spectrogram { columns, hop, bins: n / 2 + 1 }
}

const ONSET_N: usize = 512;
const ONSET_HOP: usize = 128;

/// オンセット（音の立ち上がり）の時刻を秒で返す。
///
/// スペクトルフラックス（各周波数の増加分の合計）が、周辺の平均より十分大きい極大点を拾う。
/// `sensitivity` は 0..1（大きいほど多く拾う）。
pub fn detect_onsets(mono: &[f32], sensitivity: f64) -> Vec<f64> {
    if mono.len() < ONSET_N * 2 {
        return Vec::new();
    }
    let frames = (mono.len() - ONSET_N) / ONSET_HOP + 1;
    // 立ち上がりの位置を正確に取りたいので、矩形窓のまま振幅スペクトルを求める
    let mags: Vec<Vec<f32>> = (0..frames)
        .into_par_iter()
        .map(|k| {
            let mut buf: Vec<Complex> = mono[k * ONSET_HOP..k * ONSET_HOP + ONSET_N].iter().map(|s| (*s, 0.0)).collect();
            fft(&mut buf);
            buf[..ONSET_N / 2].iter().map(|(r, i)| (1.0 + 100.0 * (r * r + i * i).sqrt() / ONSET_N as f32).ln()).collect()
        })
        .collect();
    let mut flux = vec![0.0f32; frames];
    for k in 1..frames {
        flux[k] = mags[k].iter().zip(&mags[k - 1]).map(|(a, b)| (a - b).max(0.0)).sum();
    }
    // 適応しきい値: 前後 ~100ms の平均 + 係数 × 標準偏差、かつ全体の最大に対する下限
    let radius = (0.1 * SAMPLE_RATE as f64 / ONSET_HOP as f64) as usize;
    let global_max = flux.iter().cloned().fold(0.0f32, f32::max);
    let k_std = 0.5 + 3.5 * (1.0 - sensitivity.clamp(0.0, 1.0)) as f32;
    let floor = global_max * (0.02 + 0.28 * (1.0 - sensitivity.clamp(0.0, 1.0)) as f32);
    let min_gap = (0.05 * SAMPLE_RATE as f64 / ONSET_HOP as f64) as usize;
    let mut onsets = Vec::new();
    let mut last: Option<usize> = None;
    for k in 1..frames - 1 {
        let (lo, hi) = (k.saturating_sub(radius), (k + radius + 1).min(frames));
        let window = &flux[lo..hi];
        let mean = window.iter().sum::<f32>() / window.len() as f32;
        let std = (window.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / window.len() as f32).sqrt();
        let is_peak = flux[k] > flux[k - 1] && flux[k] >= flux[k + 1];
        if is_peak && flux[k] > mean + k_std * std && flux[k] > floor && last.is_none_or(|l| k - l >= min_gap) {
            // フレーム k は「k 番目の窓に初めて入った音」を表す。その音は窓の末尾 1 ホップ内にある
            let sample = k * ONSET_HOP + ONSET_N - ONSET_HOP / 2;
            onsets.push(sample as f64 / SAMPLE_RATE as f64);
            last = Some(k);
        }
    }
    onsets
}

/// 帯域（Hz）。
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Band {
    pub low: f64,
    pub high: f64,
}

impl Band {
    pub const LOW: Band = Band { low: 20.0, high: 250.0 };
    pub const MID: Band = Band { low: 250.0, high: 4000.0 };
    pub const HIGH: Band = Band { low: 4000.0, high: 20000.0 };
    pub const FULL: Band = Band { low: 20.0, high: 20000.0 };
}

const BAND_N: usize = 4096;

/// 時刻 `center`（サンプル位置）を中心にした窓での、帯域の振幅（フルスケール正弦波 = 1.0）。
pub fn band_level(mono: &[f32], center: i64, band: Band) -> f32 {
    let start = center - BAND_N as i64 / 2;
    let window: Vec<f32> =
        (0..BAND_N as i64).map(|i| mono.get((start + i).max(0) as usize).copied().filter(|_| start + i >= 0).unwrap_or(0.0)).collect();
    let mag = magnitude_spectrum(&window);
    let hz = SAMPLE_RATE as f64 / BAND_N as f64;
    let (a, b) = (((band.low / hz).ceil() as usize).min(mag.len() - 1), ((band.high / hz).floor() as usize).min(mag.len() - 1));
    if b < a {
        return 0.0;
    }
    // 帯域内のパワーの合計（ハン窓で 1 つの正弦波が隣のビンに漏れる分を、係数 1.5 で割って戻す）
    (mag[a..=b].iter().map(|m| m * m).sum::<f32>() / 1.5).sqrt()
}

/// 帯域の音量を、コンポのフレームごとのキーフレーム（0..100）にする。
/// `offset` は音声の先頭が来るコンポ時刻。
pub fn band_keyframes(mono: &[f32], band: Band, fr: FrameRate, offset: Time, duration: Time) -> Property<f64> {
    let frames = fr.frame_count(duration);
    let keys: Vec<Keyframe<f64>> = (0..frames)
        .into_par_iter()
        .map(|f| {
            let t = fr.frame_to_time(f);
            let sample = ((t - offset).seconds() * SAMPLE_RATE as f64).round() as i64;
            Keyframe::linear(t, (band_level(mono, sample, band) * 100.0) as f64)
        })
        .collect();
    Property::constant(0.0).with_keys(keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = SAMPLE_RATE as f64;

    fn sine(freq: f64, seconds: f64, amp: f32) -> Vec<f32> {
        (0..(seconds * SR) as usize).map(|i| (2.0 * std::f64::consts::PI * freq * i as f64 / SR).sin() as f32 * amp).collect()
    }

    /// 120BPM のクリック（減衰する短いノイズ）と、その正確な時刻。
    fn clicks(bpm: f64, seconds: f64) -> (Vec<f32>, Vec<f64>) {
        let mut s = vec![0.0f32; (seconds * SR) as usize];
        let mut times = Vec::new();
        let mut t = 0.25;
        let mut seed = 12345u32;
        while t < seconds - 0.2 {
            let start = (t * SR).round() as usize;
            for i in 0..2400 {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let noise = (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0;
                s[start + i] += noise * 0.8 * (-(i as f32) / 300.0).exp();
            }
            times.push(start as f64 / SR);
            t += 60.0 / bpm;
        }
        (s, times)
    }

    #[test]
    fn peaks_capture_extremes() {
        let s = sine(100.0, 0.1, 0.5);
        let p = peaks(&s, 480);
        assert_eq!(p.len(), 10);
        assert!(p.iter().all(|(lo, hi)| (*lo + 0.5).abs() < 0.01 && (*hi - 0.5).abs() < 0.01));
    }

    #[test]
    fn spectrogram_shows_tone_at_its_bin() {
        let sp = spectrogram(&sine(3000.0, 0.5, 1.0), 1024, 256);
        assert_eq!(sp.bins, 513);
        let col = &sp.columns[sp.columns.len() / 2];
        let peak = (0..col.len()).max_by(|a, b| col[*a].total_cmp(&col[*b])).unwrap();
        assert_eq!(peak, 64);
        assert!(col[peak].abs() < 0.5 && col[200] < -60.0);
    }

    #[test]
    fn onsets_match_click_track_within_5ms() {
        let (s, times) = clicks(120.0, 20.0);
        let found = detect_onsets(&s, 0.5);
        let hits = times.iter().filter(|t| found.iter().any(|f| (f - **t).abs() <= 0.005)).count();
        assert!(hits as f64 / times.len() as f64 >= 0.98, "{hits}/{} 検出", times.len());
        // 誤検出がほとんど無い
        assert!(found.len() <= times.len() + 1, "検出 {} 個（正解 {} 個）", found.len(), times.len());
        // 無音・定常音では拾わない
        assert!(detect_onsets(&vec![0.0; 48000], 0.5).is_empty());
        assert!(detect_onsets(&sine(440.0, 2.0, 0.5)[4800..], 0.5).len() <= 1);
    }

    #[test]
    fn sensitivity_controls_how_many_onsets_are_found() {
        // 大きいクリックと小さいクリックが交互に鳴る
        let (mut s, _) = clicks(120.0, 10.0);
        let (quiet, _) = clicks(120.0, 10.0);
        for (i, q) in quiet.iter().enumerate() {
            let j = i + 12000;
            if j < s.len() {
                s[j] += q * 0.08;
            }
        }
        let low = detect_onsets(&s, 0.1).len();
        let high = detect_onsets(&s, 1.0).len();
        assert!(high > low, "感度 1.0: {high} 個、感度 0.1: {low} 個");
    }

    #[test]
    fn band_levels_respond_only_to_their_band() {
        for (freq, own) in [(100.0, Band::LOW), (1000.0, Band::MID), (8000.0, Band::HIGH)] {
            let s = sine(freq, 1.0, 0.5);
            let level = band_level(&s, 24000, own);
            assert!((level - 0.5).abs() < 0.02, "{freq}Hz: {level}");
            for other in [Band::LOW, Band::MID, Band::HIGH] {
                if other != own {
                    let l = band_level(&s, 24000, other);
                    assert!(20.0 * (l / level).max(1e-9).log10() < -40.0, "{freq}Hz が {other:?} に {l} 漏れている");
                }
            }
        }
    }

    #[test]
    fn band_keyframes_follow_the_envelope() {
        // 0.5 秒から鳴り始める 100Hz
        let mut s = vec![0.0f32; 24000];
        s.extend(sine(100.0, 1.0, 0.5));
        let fr = FrameRate::FPS_30;
        let p = band_keyframes(&s, Band::LOW, fr, Time::ZERO, Time::from_seconds(1.5));
        assert_eq!(p.keyframes.len(), 45);
        assert!(p.eval(Time::from_seconds(0.2)) < 0.5);
        assert!((p.eval(Time::from_seconds(1.0)) - 50.0).abs() < 2.0);
        // 音声の開始位置をずらすと、キーも同じだけずれる
        let shifted = band_keyframes(&s, Band::LOW, fr, Time::from_seconds(0.5), Time::from_seconds(2.0));
        assert!(shifted.eval(Time::from_seconds(0.7)) < 0.5);
        assert!((shifted.eval(Time::from_seconds(1.5)) - 50.0).abs() < 2.0);
    }
}
