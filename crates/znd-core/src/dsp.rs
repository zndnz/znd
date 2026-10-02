//! 小さな信号処理ユーティリティ（FFT・窓関数）。音声解析とオーディオ系エフェクトで共有する。

/// 複素数（実部, 虚部）。
pub type Complex = (f32, f32);

/// インプレースの基数 2 FFT。長さは 2 の冪であること。
pub fn fft(buf: &mut [Complex]) {
    let n = buf.len();
    assert!(n.is_power_of_two(), "FFT length must be a power of two");
    // ビット反転並べ替え
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            buf.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * std::f64::consts::PI / len as f64;
        let (wr, wi) = (ang.cos() as f32, ang.sin() as f32);
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (ar, ai) = buf[start + k];
                let (br, bi) = buf[start + k + len / 2];
                let (tr, ti) = (br * cr - bi * ci, br * ci + bi * cr);
                buf[start + k] = (ar + tr, ai + ti);
                buf[start + k + len / 2] = (ar - tr, ai - ti);
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
        }
        len <<= 1;
    }
}

/// ハン窓を掛けた実信号の振幅スペクトル（長さ n/2+1）。`samples.len()` は 2 の冪。
/// 振幅は「フルスケールの正弦波で 1.0」になるよう正規化する。
pub fn magnitude_spectrum(samples: &[f32]) -> Vec<f32> {
    let n = samples.len();
    let mut buf: Vec<Complex> = samples
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let w = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos();
            (s * w, 0.0)
        })
        .collect();
    fft(&mut buf);
    // ハン窓のコヒーレントゲイン 0.5、片側スペクトルなので 2/n、合わせて 4/n
    buf[..=n / 2].iter().map(|(r, i)| (r * r + i * i).sqrt() * 4.0 / n as f32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_peaks_at_its_bin_with_unit_amplitude() {
        let n = 1024;
        let bin = 37;
        let s: Vec<f32> = (0..n).map(|i| (2.0 * std::f32::consts::PI * bin as f32 * i as f32 / n as f32).sin()).collect();
        let m = magnitude_spectrum(&s);
        let peak = m.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap();
        assert_eq!(peak.0, bin);
        assert!((peak.1 - 1.0).abs() < 0.01, "{}", peak.1);
        assert!(m[bin + 5] < 0.001);
    }

    #[test]
    fn impulse_has_flat_spectrum() {
        let mut buf = vec![(0.0, 0.0); 16];
        buf[0] = (1.0, 0.0);
        fft(&mut buf);
        assert!(buf.iter().all(|(r, i)| (r - 1.0).abs() < 1e-6 && i.abs() < 1e-6));
    }
}
