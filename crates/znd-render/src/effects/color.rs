//! カラー補正系。特記のないものは表示値（sRGB 符号化後）で計算する。

use super::blur::{gaussian_buf, sigma_for};
use super::{blend_with_original, display_luma, from_display, map_display, map_pixels, to_display, FxContext};
use crate::blur::Edge;
use crate::color::luma;
use crate::frame::FrameBuf;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use znd_core::effect::EvalParams;

/// 相対パスの LUT を探す基準ディレクトリ（プロジェクトファイルの場所）。
static BASE_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn set_base_dir(dir: impl Into<PathBuf>) {
    *BASE_DIR.lock().unwrap() = Some(dir.into());
}

/// プロジェクトからの相対パスを解決する（絶対パスはそのまま）。
pub(crate) fn resolve_path(p: &str) -> PathBuf {
    let path = PathBuf::from(p);
    if path.is_relative() {
        if let Some(base) = BASE_DIR.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            return base.join(path);
        }
    }
    path
}

pub struct Lut3d {
    pub size: usize,
    /// r が最も速く回る順（.cube の並び）。
    pub data: Vec<[f32; 3]>,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
}

impl Lut3d {
    pub fn parse(src: &str) -> Result<Lut3d, String> {
        let mut size = 0usize;
        let mut data = Vec::new();
        let (mut dmin, mut dmax) = ([0.0f32; 3], [1.0f32; 3]);
        let triple = |rest: &str| -> Option<[f32; 3]> {
            let v: Vec<f32> = rest.split_whitespace().filter_map(|s| s.parse().ok()).collect();
            (v.len() == 3).then(|| [v[0], v[1], v[2]])
        };
        for line in src.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("TITLE") {
                continue;
            }
            if let Some(r) = line.strip_prefix("LUT_3D_SIZE") {
                size = r.trim().parse().map_err(|_| "LUT_3D_SIZE が不正です".to_string())?;
            } else if line.starts_with("LUT_1D_SIZE") {
                return Err("1D LUT には対応していません（3D LUT の .cube を使ってください）".into());
            } else if let Some(r) = line.strip_prefix("DOMAIN_MIN") {
                dmin = triple(r).ok_or("DOMAIN_MIN が不正です")?;
            } else if let Some(r) = line.strip_prefix("DOMAIN_MAX") {
                dmax = triple(r).ok_or("DOMAIN_MAX が不正です")?;
            } else if let Some(v) = triple(line) {
                data.push(v);
            }
        }
        if size < 2 || data.len() != size * size * size {
            return Err(format!("LUT のデータ数が合いません（サイズ {size}、データ {} 行）", data.len()));
        }
        Ok(Lut3d { size, data, domain_min: dmin, domain_max: dmax })
    }

    /// 三線形補間で引く。
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let n = self.size;
        let mut idx = [0usize; 3];
        let mut frac = [0.0f32; 3];
        for k in 0..3 {
            let u = ((c[k] - self.domain_min[k]) / (self.domain_max[k] - self.domain_min[k])).clamp(0.0, 1.0) * (n - 1) as f32;
            idx[k] = (u.floor() as usize).min(n - 2);
            frac[k] = u - idx[k] as f32;
        }
        let at = |r: usize, g: usize, b: usize| self.data[(b * n + g) * n + r];
        let lerp = |a: [f32; 3], b: [f32; 3], t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
        let (r, g, b) = (idx[0], idx[1], idx[2]);
        let c00 = lerp(at(r, g, b), at(r + 1, g, b), frac[0]);
        let c10 = lerp(at(r, g + 1, b), at(r + 1, g + 1, b), frac[0]);
        let c01 = lerp(at(r, g, b + 1), at(r + 1, g, b + 1), frac[0]);
        let c11 = lerp(at(r, g + 1, b + 1), at(r + 1, g + 1, b + 1), frac[0]);
        lerp(lerp(c00, c10, frac[1]), lerp(c01, c11, frac[1]), frac[2])
    }
}

fn load_lut(path: &str) -> Option<Arc<Lut3d>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Arc<Lut3d>>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Mutex::default);
    if let Some(hit) = cache.lock().unwrap().get(path) {
        return hit.clone();
    }
    let mut full = PathBuf::from(path);
    if full.is_relative() {
        if let Some(base) = BASE_DIR.lock().unwrap().as_ref() {
            full = base.join(path);
        }
    }
    let lut = std::fs::read_to_string(&full).ok().and_then(|s| Lut3d::parse(&s).ok()).map(Arc::new);
    cache.lock().unwrap().insert(path.to_string(), lut.clone());
    lut
}

/// 制御点を通る単調な区分線形カーブ。
fn curve_value(points: &[[f64; 2]], x: f32) -> f32 {
    let mut pts: Vec<[f64; 2]> = points.to_vec();
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let (Some(first), Some(last)) = (pts.first(), pts.last()) else { return x };
    let x = x as f64;
    if x <= first[0] {
        return first[1] as f32;
    }
    if x >= last[0] {
        return last[1] as f32;
    }
    let i = pts.partition_point(|p| p[0] <= x).clamp(1, pts.len() - 1);
    let (a, b) = (pts[i - 1], pts[i]);
    let t = if b[0] > a[0] { (x - a[0]) / (b[0] - a[0]) } else { 0.0 };
    // 区間内は smoothstep ではなく直線。滑らかさが欲しければ制御点を増やす
    (a[1] + (b[1] - a[1]) * t) as f32
}

fn rgb_to_hsl(c: [f32; 3]) -> [f32; 3] {
    let (mx, mn) = (c[0].max(c[1]).max(c[2]), c[0].min(c[1]).min(c[2]));
    let l = (mx + mn) / 2.0;
    let d = mx - mn;
    if d <= 1e-9 {
        return [0.0, 0.0, l];
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs()).max(1e-9);
    let h = if mx == c[0] {
        ((c[1] - c[2]) / d).rem_euclid(6.0)
    } else if mx == c[1] {
        (c[2] - c[0]) / d + 2.0
    } else {
        (c[0] - c[1]) / d + 4.0
    };
    [h * 60.0, s.min(1.0), l]
}

fn hsl_to_rgb(c: [f32; 3]) -> [f32; 3] {
    let (h, s, l) = (c[0].rem_euclid(360.0) / 60.0, c[1], c[2]);
    let chroma = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = chroma * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = l - chroma / 2.0;
    [r + m, g + m, b + m]
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// 色パラメータ（sRGB）を表示値の RGB として取り出す。
fn disp(c: [f64; 4]) -> [f32; 3] {
    [c[0] as f32, c[1] as f32, c[2] as f32]
}

pub fn apply(kind: &str, p: &EvalParams, mut buf: FrameBuf, ctx: &FxContext) -> FrameBuf {
    match kind {
        "levels" => {
            let (ib, iw) = (p.f("in_black") as f32, p.f("in_white") as f32);
            let (ob, ow) = (p.f("out_black") as f32, p.f("out_white") as f32);
            let inv_gamma = 1.0 / (p.f("gamma") as f32).max(0.01);
            let range = (iw - ib).abs().max(1e-6) * (iw - ib).signum();
            map_display(&mut buf, |c| {
                c.map(|v| {
                    let n = ((v - ib) / range).max(0.0);
                    // 1 を超える値（HDR）はガンマを掛けずに傾きを保って伸ばす
                    let g = if n <= 1.0 { n.powf(inv_gamma) } else { n };
                    ob + g * (ow - ob)
                })
            });
            buf
        }
        "curves" => {
            let pts = p.curve("curve").to_vec();
            map_display(&mut buf, |c| c.map(|v| curve_value(&pts, v)));
            buf
        }
        "hue_saturation" => {
            let (dh, ds, dl) = (p.f("hue") as f32, p.f("saturation") as f32 / 100.0, p.f("lightness") as f32 / 100.0);
            map_display(&mut buf, |c| {
                let mut hsl = rgb_to_hsl(c.map(|v| v.clamp(0.0, 1.0)));
                hsl[0] += dh;
                hsl[1] = (hsl[1] * (1.0 + ds)).clamp(0.0, 1.0);
                // 明度: 正は白へ、負は黒へ寄せる
                hsl[2] = if dl >= 0.0 { hsl[2] + (1.0 - hsl[2]) * dl } else { hsl[2] * (1.0 + dl) };
                hsl_to_rgb(hsl)
            });
            buf
        }
        "color_balance" => {
            let get = |a: &str, b: &str, c: &str| [p.f(a) as f32 / 100.0, p.f(b) as f32 / 100.0, p.f(c) as f32 / 100.0];
            let (sh, mid, hi) =
                (get("shadow_r", "shadow_g", "shadow_b"), get("mid_r", "mid_g", "mid_b"), get("high_r", "high_g", "high_b"));
            map_display(&mut buf, |c| {
                let l = luma(c).clamp(0.0, 1.0);
                // 明るさ帯ごとの重み（合計がおおむね 1 になる山形）
                let ws = (1.0 - l * 2.0).max(0.0);
                let wh = (l * 2.0 - 1.0).max(0.0);
                let wm = 1.0 - ws - wh;
                [0, 1, 2].map(|k| c[k] + (sh[k] * ws + mid[k] * wm + hi[k] * wh) * 0.5)
            });
            buf
        }
        "lut" => {
            let Some(lut) = load_lut(p.s("path")) else { return buf };
            let k = (p.f("intensity") as f32 / 100.0).clamp(0.0, 1.0);
            map_display(&mut buf, |c| lerp3(c, lut.apply(c), k));
            buf
        }
        "tritone" => {
            let (hi, mid, sh) = (disp(p.c("highlights")), disp(p.c("midtones")), disp(p.c("shadows")));
            let original = buf.clone();
            map_display(&mut buf, |c| {
                let l = luma(c).clamp(0.0, 1.0);
                if l < 0.5 {
                    lerp3(sh, mid, l * 2.0)
                } else {
                    lerp3(mid, hi, l * 2.0 - 1.0)
                }
            });
            blend_with_original(&original, buf, p.f("blend"))
        }
        "gradient_map" => {
            let (a, b) = (disp(p.c("color_a")), disp(p.c("color_b")));
            let original = buf.clone();
            map_display(&mut buf, |c| lerp3(a, b, luma(c).clamp(0.0, 1.0)));
            blend_with_original(&original, buf, p.f("blend"))
        }
        "shadow_highlight" => {
            let (sa, ha) = (p.f("shadow_amount") as f32 / 100.0, p.f("highlight_amount") as f32 / 100.0);
            // 周辺の明るさ（ぼかした輝度）で暗部・明部を判定する
            let mut local = buf.clone();
            let sg = sigma_for(p.f("radius"), ctx.scale);
            gaussian_buf(&mut local, sg, sg, Edge::Clamp);
            map_pixels(&mut buf, |x, y, px| {
                if px[3] <= 0.0 {
                    return px;
                }
                let l = display_luma(local.pixel(x, y)).clamp(0.0, 1.0);
                let d = to_display(px);
                let lift = sa * (1.0 - l).powi(2);
                let cut = ha * l.powi(2);
                from_display(d.map(|v| v + (1.0 - v) * lift * 0.6 - v * cut * 0.6), px[3])
            });
            buf
        }
        "exposure" => {
            // 露出は光量そのものの操作なのでリニア値に掛ける
            let (gain, off) = (2f32.powf(p.f("stops") as f32), p.f("offset") as f32);
            map_pixels(&mut buf, |_, _, px| [px[0] * gain + off * px[3], px[1] * gain + off * px[3], px[2] * gain + off * px[3], px[3]]);
            buf
        }
        "brightness_contrast" => {
            let (b, c) = (p.f("brightness") as f32 / 255.0, p.f("contrast") as f32 / 100.0);
            let k = if c >= 0.0 { 1.0 / (1.0 - c).max(0.01) } else { 1.0 + c };
            map_display(&mut buf, |col| col.map(|v| (v + b - 0.5) * k + 0.5));
            buf
        }
        "invert" => {
            let original = buf.clone();
            map_display(&mut buf, |c| c.map(|v| 1.0 - v));
            blend_with_original(&original, buf, p.f("blend"))
        }
        _ => buf,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_identity_lut_is_identity() {
        let n = 5;
        let mut s = format!("TITLE \"id\"\nLUT_3D_SIZE {n}\n");
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    s += &format!("{} {} {}\n", r as f32 / 4.0, g as f32 / 4.0, b as f32 / 4.0);
                }
            }
        }
        let lut = Lut3d::parse(&s).unwrap();
        for c in [[0.0, 0.0, 0.0], [0.13, 0.57, 0.91], [1.0, 1.0, 1.0], [0.25, 0.5, 0.75]] {
            let o = lut.apply(c);
            assert!((0..3).all(|k| (o[k] - c[k]).abs() < 1e-5), "{c:?} -> {o:?}");
        }
        assert!(Lut3d::parse("LUT_3D_SIZE 2\n0 0 0\n").is_err());
    }

    #[test]
    fn hsl_roundtrip() {
        for c in [[0.2, 0.4, 0.6], [1.0, 0.0, 0.0], [0.5, 0.5, 0.5], [0.9, 0.8, 0.1]] {
            let o = hsl_to_rgb(rgb_to_hsl(c));
            assert!((0..3).all(|k| (o[k] - c[k]).abs() < 1e-5), "{c:?} -> {o:?}");
        }
        assert!((rgb_to_hsl([1.0, 0.0, 0.0])[0]).abs() < 1e-5);
        assert!((rgb_to_hsl([0.0, 0.0, 1.0])[0] - 240.0).abs() < 1e-3);
    }
}
