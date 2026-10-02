//! タイムストレッチ（WSOLA）、リサンプル、ピッチシフト。入出力はインターリーブのステレオ。

/// 4 点エルミート補間で `pos`（フレーム単位、小数可）の値を読む。範囲外は 0。
fn hermite(data: &[f32], ch: usize, channels: usize, pos: f64) -> f32 {
    let frames = (data.len() / channels) as isize;
    let i = pos.floor() as isize;
    let f = (pos - i as f64) as f32;
    let at = |k: isize| -> f32 {
        if k < 0 || k >= frames {
            0.0
        } else {
            data[k as usize * channels + ch]
        }
    };
    let (y0, y1, y2, y3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
    let c0 = y1;
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * f + c2) * f + c1) * f + c0
}

/// 再生速度 `rate` 倍で読み直す（尺は 1/rate、ピッチも rate 倍になる）。
pub fn resample(data: &[f32], channels: usize, rate: f64) -> Vec<f32> {
    let frames = data.len() / channels;
    let out_frames = (frames as f64 / rate).round() as usize;
    let mut out = vec![0.0f32; out_frames * channels];
    for n in 0..out_frames {
        for ch in 0..channels {
            out[n * channels + ch] = hermite(data, ch, channels, n as f64 * rate);
        }
    }
    out
}

/// 任意位置の読み出し（タイムリマップ用）。
pub fn sample_at(data: &[f32], channels: usize, ch: usize, pos: f64) -> f32 {
    hermite(data, ch, channels, pos)
}

const FRAME: usize = 2048;
const HOP: usize = FRAME / 2;
const SEARCH: isize = 512;
/// 相関を取るときの間引き。位置合わせの精度には十分で、計算量が 1/4 になる。
const CORR_STEP: usize = 4;

/// ピッチを変えずに再生速度を `speed` 倍にする（尺は 1/speed）。WSOLA。
pub fn time_stretch(data: &[f32], channels: usize, speed: f64) -> Vec<f32> {
    let frames = data.len() / channels;
    let out_frames = (frames as f64 / speed).round() as usize;
    if (speed - 1.0).abs() < 1e-9 || frames < FRAME * 2 {
        // 短すぎる素材は窓が取れないので、単純な読み直しで尺だけ合わせる
        return if (speed - 1.0).abs() < 1e-9 { data.to_vec() } else { resample(data, channels, speed) };
    }
    // 位置合わせ用のモノラル
    let mono: Vec<f32> = (0..frames).map(|i| (0..channels).map(|c| data[i * channels + c]).sum::<f32>() / channels as f32).collect();
    let window: Vec<f32> = (0..FRAME).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / FRAME as f32).cos()).collect();
    let mut out = vec![0.0f32; (out_frames + FRAME) * channels];
    let get = |i: isize| -> f32 {
        if i < 0 || i as usize >= frames {
            0.0
        } else {
            mono[i as usize]
        }
    };

    // prev: 直前に採用した入力フレームの位置。その「自然な続き」(prev + HOP) に最も似た位置を探す
    let mut prev: isize = 0;
    let mut k = 0usize;
    loop {
        let out_pos = k * HOP;
        if out_pos >= out_frames {
            break;
        }
        let ideal = (out_pos as f64 * speed).round() as isize;
        let chosen = if k == 0 {
            0
        } else {
            let target = prev + HOP as isize;
            let mut best = (f32::MIN, 0isize);
            let mut d = -SEARCH;
            while d <= SEARCH {
                let cand = ideal + d;
                let mut corr = 0.0f32;
                let mut i = 0;
                while i < HOP {
                    corr += get(cand + i as isize) * get(target + i as isize);
                    i += CORR_STEP;
                }
                if corr > best.0 {
                    best = (corr, d);
                }
                d += 2;
            }
            ideal + best.1
        };
        for i in 0..FRAME {
            let src = chosen + i as isize;
            if src < 0 || src as usize >= frames {
                continue;
            }
            let w = window[i];
            for c in 0..channels {
                out[(out_pos + i) * channels + c] += data[src as usize * channels + c] * w;
            }
        }
        prev = chosen;
        k += 1;
    }
    // ハン窓を 50% 重ねると和は 1。先頭の半フレームだけは窓が 1 枚なので補正する
    for i in 0..HOP.min(out_frames) {
        let w = window[i].max(1e-3);
        for c in 0..channels {
            out[i * channels + c] /= w;
        }
    }
    out.truncate(out_frames * channels);
    out
}

/// 尺を変えずにピッチを `semitones` 半音ずらす。
pub fn pitch_shift(data: &[f32], channels: usize, semitones: f64) -> Vec<f32> {
    if semitones.abs() < 1e-9 {
        return data.to_vec();
    }
    let ratio = 2f64.powf(semitones / 12.0);
    // 尺を ratio 倍に伸ばしてから ratio 倍速で読むと、尺は元のまま音程だけ ratio 倍になる
    let stretched = time_stretch(data, channels, 1.0 / ratio);
    let mut out = resample(&stretched, channels, ratio);
    out.resize(data.len(), 0.0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use znd_core::dsp::magnitude_spectrum;

    const SR: f64 = 48_000.0;

    fn sine(freq: f64, seconds: f64) -> Vec<f32> {
        let n = (seconds * SR) as usize;
        (0..n)
            .flat_map(|i| {
                let v = (2.0 * std::f64::consts::PI * freq * i as f64 / SR).sin() as f32 * 0.5;
                [v, v]
            })
            .collect()
    }

    /// 中央付近 16384 サンプルのスペクトルから基本周波数を求める（放物線補間）。
    fn fundamental(data: &[f32]) -> f64 {
        let n = 16384;
        let start = (data.len() / 2 / 2).saturating_sub(n / 2);
        let mono: Vec<f32> = (0..n).map(|i| data[(start + i) * 2]).collect();
        let m = magnitude_spectrum(&mono);
        let peak = (1..m.len() - 1).max_by(|a, b| m[*a].total_cmp(&m[*b])).unwrap();
        let (a, b, c) = (m[peak - 1] as f64, m[peak] as f64, m[peak + 1] as f64);
        let delta = 0.5 * (a - c) / (a - 2.0 * b + c);
        (peak as f64 + delta) * SR / n as f64
    }

    fn rms(data: &[f32]) -> f64 {
        (data.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / data.len() as f64).sqrt()
    }

    #[test]
    fn resample_changes_pitch_and_length() {
        let s = sine(440.0, 1.0);
        let up = resample(&s, 2, 2.0);
        assert_eq!(up.len(), s.len() / 2);
        assert!((fundamental(&up) - 880.0).abs() < 2.0);
    }

    #[test]
    fn time_stretch_keeps_pitch_and_hits_exact_length() {
        let s = sine(440.0, 2.0);
        for speed in [0.5, 0.8, 1.25, 2.0] {
            let out = time_stretch(&s, 2, speed);
            let expect = (s.len() as f64 / 2.0 / speed).round() as usize * 2;
            assert_eq!(out.len(), expect, "speed {speed}");
            let f = fundamental(&out);
            assert!((f - 440.0).abs() / 440.0 < 0.01, "speed {speed}: {f} Hz");
            // 音量が保たれる（継ぎ目で打ち消し合っていない）
            let mid = &out[out.len() / 4..out.len() * 3 / 4];
            assert!((rms(mid) / rms(&s) - 1.0).abs() < 0.08, "speed {speed}: rms ratio {}", rms(mid) / rms(&s));
        }
        assert_eq!(time_stretch(&s, 2, 1.0), s);
    }

    #[test]
    fn pitch_shift_keeps_length() {
        let s = sine(440.0, 2.0);
        for semi in [-12.0, -5.0, 7.0, 12.0] {
            let out = pitch_shift(&s, 2, semi);
            assert_eq!(out.len(), s.len());
            let expect = 440.0 * 2f64.powf(semi / 12.0);
            let f = fundamental(&out);
            assert!((f - expect).abs() / expect < 0.01, "{semi} 半音: {f} Hz（期待 {expect}）");
        }
    }
}
