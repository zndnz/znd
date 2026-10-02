//! 伝達関数と輝度係数。作業色空間は sRGB/Rec.709 原色のリニアライト。

use znd_core::ColorSpace;

/// Rec.709 の輝度係数。
pub const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

pub fn luma(rgb: [f32; 3]) -> f32 {
    rgb[0] * LUMA[0] + rgb[1] * LUMA[1] + rgb[2] * LUMA[2]
}

/// sRGB EOTF。負値・1 超は符号を保って拡張する。
pub fn srgb_to_linear(c: f32) -> f32 {
    let a = c.abs();
    let l = if a <= 0.04045 { a / 12.92 } else { ((a + 0.055) / 1.055).powf(2.4) };
    l.copysign(c)
}

/// sRGB OETF（`srgb_to_linear` の逆）。
pub fn linear_to_srgb(c: f32) -> f32 {
    let a = c.abs();
    let s = if a <= 0.003_130_8 { a * 12.92 } else { 1.055 * a.powf(1.0 / 2.4) - 0.055 };
    s.copysign(c)
}

/// sRGB 表示値の色をリニアに変換する。
pub fn srgb3_to_linear(c: [f32; 3]) -> [f32; 3] {
    [srgb_to_linear(c[0]), srgb_to_linear(c[1]), srgb_to_linear(c[2])]
}

pub type Mat3 = [[f32; 3]; 3];

pub const IDENTITY: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// sRGB（Rec.709 原色・D65）リニア → 各色空間のリニア。
fn from_srgb(space: ColorSpace) -> Mat3 {
    match space {
        ColorSpace::Srgb | ColorSpace::Rec709 => IDENTITY,
        ColorSpace::DisplayP3 => [[0.822_462, 0.177_538, 0.0], [0.033_194, 0.966_806, 0.0], [0.017_083, 0.072_397, 0.910_520]],
        // AP1（D60）。ブラッドフォード順応込み
        ColorSpace::AcesCg => [[0.613_097, 0.339_523, 0.047_379], [0.070_194, 0.916_354, 0.013_452], [0.020_616, 0.109_570, 0.869_815]],
    }
}

pub fn mat_mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut o = [[0.0; 3]; 3];
    for (i, row) in o.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    o
}

pub fn mat_inv(m: &Mat3) -> Mat3 {
    let c = |i: usize, j: usize| {
        m[(i + 1) % 3][(j + 1) % 3] * m[(i + 2) % 3][(j + 2) % 3] - m[(i + 1) % 3][(j + 2) % 3] * m[(i + 2) % 3][(j + 1) % 3]
    };
    let det = m[0][0] * c(0, 0) + m[0][1] * c(0, 1) + m[0][2] * c(0, 2);
    let mut o = [[0.0; 3]; 3];
    for (i, row) in o.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = c(j, i) / det;
        }
    }
    o
}

pub fn mat_apply(m: &Mat3, c: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * c[0] + m[0][1] * c[1] + m[0][2] * c[2],
        m[1][0] * c[0] + m[1][1] * c[1] + m[1][2] * c[2],
        m[2][0] * c[0] + m[2][1] * c[1] + m[2][2] * c[2],
    ]
}

/// 入力（sRGB 前提の素材・色指定）を作業色空間へ運ぶ変換。
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct ColorXf {
    pub to_working: Mat3,
    pub identity: bool,
}

impl ColorXf {
    pub const SRGB: ColorXf = ColorXf { to_working: IDENTITY, identity: true };

    pub fn new(working: ColorSpace) -> Self {
        ColorXf { to_working: from_srgb(working), identity: matches!(working, ColorSpace::Srgb | ColorSpace::Rec709) }
    }

    /// sRGB 表示値の色 → 作業空間リニア。
    pub fn rgb(&self, c: [f32; 3]) -> [f32; 3] {
        let l = srgb3_to_linear(c);
        if self.identity {
            l
        } else {
            mat_apply(&self.to_working, l)
        }
    }

    /// sRGB の RGBA → 作業空間リニア・プリマルチプライド。
    pub fn premul(&self, c: [f64; 4]) -> [f32; 4] {
        let a = c[3].clamp(0.0, 1.0) as f32;
        let l = self.rgb([c[0] as f32, c[1] as f32, c[2] as f32]);
        [l[0] * a, l[1] * a, l[2] * a, a]
    }

    /// sRGB リニア・プリマルチプライドの画像を作業空間へ変換する（行列は線形なのでそのまま掛けられる）。
    pub fn convert_buffer(&self, data: &mut [f32]) {
        if self.identity {
            return;
        }
        for px in data.chunks_exact_mut(4) {
            let c = mat_apply(&self.to_working, [px[0], px[1], px[2]]);
            px[..3].copy_from_slice(&c);
        }
    }
}

/// 作業色空間 → 出力色空間の変換（原色の行列＋伝達関数）。
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct OutputXf {
    pub matrix: Mat3,
    pub identity: bool,
    pub space: ColorSpace,
}

impl OutputXf {
    pub const SRGB: OutputXf = OutputXf { matrix: IDENTITY, identity: true, space: ColorSpace::Srgb };

    pub fn new(working: ColorSpace, output: ColorSpace) -> Self {
        let matrix = mat_mul(&from_srgb(output), &mat_inv(&from_srgb(working)));
        let same = from_srgb(working) == from_srgb(output);
        OutputXf { matrix, identity: same, space: output }
    }

    /// 作業空間リニア → 出力の符号化値（0..1）。
    pub fn encode(&self, c: [f32; 3]) -> [f32; 3] {
        let c = if self.identity { c } else { mat_apply(&self.matrix, c) };
        match self.space {
            ColorSpace::Srgb | ColorSpace::DisplayP3 => [linear_to_srgb(c[0]), linear_to_srgb(c[1]), linear_to_srgb(c[2])],
            // BT.1886（ガンマ 2.4）のディスプレイで意図どおりに見える符号化
            ColorSpace::Rec709 => c.map(|v| v.max(0.0).powf(1.0 / 2.4)),
            // ACEScg はリニアのまま（EXR 出力向け）
            ColorSpace::AcesCg => c,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_space_matrices() {
        // 白は白のまま
        for space in [ColorSpace::DisplayP3, ColorSpace::AcesCg] {
            let w = ColorXf::new(space).rgb([1.0, 1.0, 1.0]);
            assert!(w.iter().all(|v| (v - 1.0).abs() < 1e-4), "{space:?} {w:?}");
            // 作業空間 → sRGB 出力で元に戻る
            let out = OutputXf::new(space, ColorSpace::Srgb);
            let c = ColorXf::new(space).rgb([0.8, 0.3, 0.1]);
            let back = out.encode(c);
            assert!((back[0] - 0.8).abs() < 1e-3 && (back[1] - 0.3).abs() < 1e-3 && (back[2] - 0.1).abs() < 1e-3, "{back:?}");
        }
        // sRGB の純赤は P3 では彩度が下がって見える値になる
        let p3 = ColorXf::new(ColorSpace::DisplayP3).rgb([1.0, 0.0, 0.0]);
        assert!(p3[0] < 1.0 && p3[1] > 0.0);
        assert!(OutputXf::new(ColorSpace::Srgb, ColorSpace::Rec709).identity);
        let inv = mat_inv(&from_srgb(ColorSpace::AcesCg));
        let id = mat_mul(&inv, &from_srgb(ColorSpace::AcesCg));
        assert!((id[0][0] - 1.0).abs() < 1e-5 && id[0][1].abs() < 1e-5);
    }

    #[test]
    fn transfer_roundtrip() {
        for i in 0..=1000 {
            let v = i as f32 / 500.0 - 0.5;
            assert!((linear_to_srgb(srgb_to_linear(v)) - v).abs() < 1e-5);
        }
        assert!((srgb_to_linear(0.5) - 0.214_041).abs() < 1e-5);
        assert_eq!(srgb_to_linear(0.0), 0.0);
        assert!((srgb_to_linear(1.0) - 1.0).abs() < 1e-6);
    }
}
