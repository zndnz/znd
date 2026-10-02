//! 作業空間（リニア f32）から表示・書き出し用の 8bit sRGB への変換。
//!
//! 量子化前に ±0.5LSB の一様ノイズ（画素位置とフレームから決まるハッシュ）を足して
//! グラデーションのバンディングを防ぐ。8bit でちょうど表現できる値（純黒・純白を含む）は
//! ノイズで動かないので、黒が浮くことはない。

use crate::color::OutputXf;
use crate::frame::FrameBuf;
use rayon::prelude::*;

fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

/// [-0.5, 0.5) の決定的ノイズ。
fn noise(x: usize, y: usize, channel: u32, seed: u32) -> f32 {
    let h = hash((x as u32).wrapping_mul(0x9e37_79b1) ^ hash((y as u32).wrapping_add(seed.wrapping_mul(0x85eb_ca6b)) ^ channel));
    (h >> 8) as f32 / (1u32 << 24) as f32 - 0.5
}

fn quantize(encoded: f32, n: f32) -> u8 {
    let v = encoded.clamp(0.0, 1.0) * 255.0;
    // ノイズ幅を僅かに狭め、伝達関数の丸め誤差があっても整数ちょうどの値が隣のコードに落ちないようにする
    (v + n * 0.99 + 0.5).floor().clamp(0.0, 255.0) as u8
}

fn convert(buf: &FrameBuf, dither_seed: Option<u32>, channels: usize, xf: &OutputXf) -> Vec<u8> {
    let mut out = vec![0u8; buf.width * buf.height * channels];
    out.par_chunks_mut(buf.width * channels).zip(buf.data.par_chunks(buf.width * 4)).enumerate().for_each(|(y, (orow, irow))| {
        for (x, (o, p)) in orow.chunks_exact_mut(channels).zip(irow.chunks_exact(4)).enumerate() {
            // RGBA 出力はストレートアルファ、RGB 出力は黒背景に合成済みの値
            let a = p[3];
            let div = if channels == 4 && a > 0.0 { a } else { 1.0 };
            let enc = xf.encode([p[0] / div, p[1] / div, p[2] / div]);
            for c in 0..3 {
                let n = dither_seed.map(|s| noise(x, y, c as u32, s)).unwrap_or(0.0);
                o[c] = quantize(enc[c], n);
            }
            if channels == 4 {
                o[3] = (a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
        }
    });
    out
}

/// sRGB 8bit・ストレートアルファの RGBA。`dither_seed` は通常フレーム番号。
pub fn to_rgba8(buf: &FrameBuf, dither_seed: Option<u32>) -> Vec<u8> {
    convert(buf, dither_seed, 4, &OutputXf::SRGB)
}

/// sRGB 8bit の RGB（アルファは破棄。不透明な画像を想定）。
pub fn to_rgb8(buf: &FrameBuf, dither_seed: Option<u32>) -> Vec<u8> {
    convert(buf, dither_seed, 3, &OutputXf::SRGB)
}

/// 出力色空間へ変換した 8bit RGBA（ストレートアルファ）。
pub fn encode_rgba8(buf: &FrameBuf, dither_seed: Option<u32>, xf: &OutputXf) -> Vec<u8> {
    convert(buf, dither_seed, 4, xf)
}

/// 出力色空間へ変換した 8bit RGB。
pub fn encode_rgb8(buf: &FrameBuf, dither_seed: Option<u32>, xf: &OutputXf) -> Vec<u8> {
    convert(buf, dither_seed, 3, xf)
}

/// 出力色空間へ変換した 16bit RGBA（ストレートアルファ、ネイティブエンディアン）。高ビット深度の書き出し用。
pub fn encode_rgba16(buf: &FrameBuf, xf: &OutputXf) -> Vec<u16> {
    let mut out = vec![0u16; buf.width * buf.height * 4];
    out.par_chunks_mut(buf.width * 4).zip(buf.data.par_chunks(buf.width * 4)).for_each(|(orow, irow)| {
        for (o, p) in orow.chunks_exact_mut(4).zip(irow.chunks_exact(4)) {
            let a = p[3];
            let div = if a > 0.0 { a } else { 1.0 };
            let enc = xf.encode([p[0] / div, p[1] / div, p[2] / div]);
            for c in 0..3 {
                o[c] = (enc[c].clamp(0.0, 1.0) * 65535.0 + 0.5) as u16;
            }
            o[3] = (a.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16;
        }
    });
    out
}

/// 出力色空間の原色へ変換したリニア RGBA（プリマルチプライドのまま）。EXR 書き出し用。
pub fn linear_output(buf: &FrameBuf, xf: &OutputXf) -> Vec<f32> {
    let mut out = buf.data.clone();
    if !xf.identity {
        for px in out.chunks_exact_mut(4) {
            let c = crate::color::mat_apply(&xf.matrix, [px[0], px[1], px[2]]);
            px[..3].copy_from_slice(&c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::srgb_to_linear;

    #[test]
    fn exact_codes_are_not_disturbed_by_dither() {
        let mut buf = FrameBuf::new(64, 64);
        for (i, px) in buf.data.chunks_exact_mut(4).enumerate() {
            let code = (i % 256) as f32;
            let v = srgb_to_linear(code / 255.0);
            px.copy_from_slice(&[v, v, v, 1.0]);
        }
        let out = to_rgba8(&buf, Some(7));
        for (i, px) in out.chunks_exact(4).enumerate() {
            assert_eq!(px[0] as usize, i % 256);
            assert_eq!(px[3], 255);
        }
    }

    #[test]
    fn dither_preserves_mean_of_in_between_values() {
        // コード 100 と 101 の 30% 地点。ディザなしなら全画素 100 に丸まる。
        let v = srgb_to_linear(100.3 / 255.0);
        let mut buf = FrameBuf::new(256, 256);
        for px in buf.data.chunks_exact_mut(4) {
            px.copy_from_slice(&[v, v, v, 1.0]);
        }
        let plain = to_rgb8(&buf, None);
        assert!(plain.iter().all(|c| *c == 100));
        let dithered = to_rgb8(&buf, Some(3));
        assert!(dithered.iter().all(|c| *c == 100 || *c == 101));
        let mean = dithered.iter().map(|c| *c as f64).sum::<f64>() / dithered.len() as f64;
        assert!((mean - 100.3).abs() < 0.02, "mean {mean}");
    }

    #[test]
    fn rgba_is_unpremultiplied() {
        let mut buf = FrameBuf::new(1, 1);
        buf.set_pixel(0, 0, [0.5, 0.0, 0.0, 0.5]);
        assert_eq!(to_rgba8(&buf, None), vec![255, 0, 0, 128]);
    }
}
