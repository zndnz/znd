//! ガウスぼかし。マスクのフェザーと各種エフェクトの基礎。
//!
//! σ が小さいときは真のガウスカーネル、大きいときはボックスブラー 3 回の近似（O(画素数)）。

use rayon::prelude::*;

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum Edge {
    /// 画像の外は透明（0）。
    Zero,
    /// 端の画素を延長する。
    Clamp,
}

fn sample(row: &[f32], n: usize, ch: usize, i: isize, c: usize, edge: Edge) -> f32 {
    if i >= 0 && (i as usize) < n {
        row[i as usize * ch + c]
    } else {
        match edge {
            Edge::Zero => 0.0,
            Edge::Clamp => row[i.clamp(0, n as isize - 1) as usize * ch + c],
        }
    }
}

fn kernel_row(src: &[f32], dst: &mut [f32], n: usize, ch: usize, kernel: &[f32], edge: Edge) {
    let r = (kernel.len() / 2) as isize;
    for i in 0..n {
        for c in 0..ch {
            let mut acc = 0.0;
            for (k, w) in kernel.iter().enumerate() {
                acc += w * sample(src, n, ch, i as isize + k as isize - r, c, edge);
            }
            dst[i * ch + c] = acc;
        }
    }
}

/// 幅 `2r+1` のボックスブラー（スライディングウィンドウ）。
fn box_row(src: &[f32], dst: &mut [f32], n: usize, ch: usize, r: usize, edge: Edge) {
    if r == 0 {
        dst.copy_from_slice(src);
        return;
    }
    let inv = 1.0 / (2 * r + 1) as f32;
    for c in 0..ch {
        // 画像の外の値（左端・右端）
        let (lo, hi) = match edge {
            Edge::Zero => (0.0, 0.0),
            Edge::Clamp => (src[c], src[(n - 1) * ch + c]),
        };
        // 最初の窓 [-r, r]
        let mut acc = lo * r as f32;
        for k in 0..=r {
            acc += if k < n { src[k * ch + c] } else { hi };
        }
        for i in 0..n {
            dst[i * ch + c] = acc * inv;
            let add = i + r + 1;
            let incoming = if add < n { src[add * ch + c] } else { hi };
            let outgoing = if i >= r { src[(i - r) * ch + c] } else { lo };
            acc += incoming - outgoing;
        }
    }
}

/// σ を 3 回のボックスブラーで近似するときの各半径。
pub fn box_radii(sigma: f32) -> [usize; 3] {
    let n = 3.0f32;
    let w_ideal = (12.0 * sigma * sigma / n + 1.0).sqrt();
    let mut wl = w_ideal.floor() as i32;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wu = wl + 2;
    let m = ((12.0 * sigma * sigma - n * (wl * wl) as f32 - 4.0 * n * wl as f32 - 3.0 * n) / (-4.0 * wl as f32 - 4.0)).round() as i32;
    let mut out = [0usize; 3];
    for (i, o) in out.iter_mut().enumerate() {
        let w = if (i as i32) < m { wl } else { wu };
        *o = ((w - 1) / 2).max(0) as usize;
    }
    out
}

fn blur_rows(data: &mut [f32], w: usize, ch: usize, sigma: f32, edge: Edge) {
    if sigma <= 0.01 || w == 0 {
        return;
    }
    if sigma < 2.5 {
        let r = (sigma * 3.0).ceil() as isize;
        let mut kernel: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
        let sum: f32 = kernel.iter().sum();
        kernel.iter_mut().for_each(|k| *k /= sum);
        data.par_chunks_mut(w * ch).for_each(|row| {
            let src = row.to_vec();
            kernel_row(&src, row, w, ch, &kernel, edge);
        });
    } else {
        let radii = box_radii(sigma);
        data.par_chunks_mut(w * ch).for_each(|row| {
            let mut tmp = row.to_vec();
            for (i, r) in radii.iter().enumerate() {
                if i % 2 == 0 {
                    box_row(row, &mut tmp, w, ch, *r, edge);
                } else {
                    box_row(&tmp, row, w, ch, *r, edge);
                }
            }
            // 3 回目の結果は tmp 側にある
            row.copy_from_slice(&tmp);
        });
    }
}

/// 縦方向のボックスブラー。行単位で足し引きするので、転置せずにキャッシュ効率よく処理できる。
fn box_cols(src: &[f32], dst: &mut [f32], stride: usize, h: usize, r: usize, edge: Edge) {
    if r == 0 {
        dst.copy_from_slice(src);
        return;
    }
    let inv = 1.0 / (2 * r + 1) as f32;
    let row = |y: usize| &src[y * stride..(y + 1) * stride];
    let clamp = edge == Edge::Clamp;
    let mut acc = vec![0.0f32; stride];
    if clamp {
        acc.iter_mut().zip(row(0)).for_each(|(a, v)| *a = v * r as f32);
    }
    for k in 0..=r {
        if k < h {
            acc.iter_mut().zip(row(k)).for_each(|(a, v)| *a += v);
        } else if clamp {
            acc.iter_mut().zip(row(h - 1)).for_each(|(a, v)| *a += v);
        }
    }
    for y in 0..h {
        dst[y * stride..(y + 1) * stride].iter_mut().zip(&acc).for_each(|(d, a)| *d = a * inv);
        let add = y + r + 1;
        if add < h {
            acc.iter_mut().zip(row(add)).for_each(|(a, v)| *a += v);
        } else if clamp {
            acc.iter_mut().zip(row(h - 1)).for_each(|(a, v)| *a += v);
        }
        if y >= r {
            acc.iter_mut().zip(row(y - r)).for_each(|(a, v)| *a -= v);
        } else if clamp {
            acc.iter_mut().zip(row(0)).for_each(|(a, v)| *a -= v);
        }
    }
}

fn blur_cols(data: &mut [f32], w: usize, h: usize, ch: usize, sigma: f32, edge: Edge) {
    if sigma <= 0.01 || h == 0 {
        return;
    }
    let stride = w * ch;
    if sigma < 2.5 {
        let r = (sigma * 3.0).ceil() as isize;
        let mut kernel: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
        let sum: f32 = kernel.iter().sum();
        kernel.iter_mut().for_each(|k| *k /= sum);
        let src = data.to_vec();
        data.par_chunks_mut(stride).enumerate().for_each(|(y, out)| {
            out.fill(0.0);
            for (k, wgt) in kernel.iter().enumerate() {
                let yy = y as isize + k as isize - r;
                let yy = if yy < 0 || yy >= h as isize {
                    if edge == Edge::Zero {
                        continue;
                    }
                    yy.clamp(0, h as isize - 1)
                } else {
                    yy
                } as usize;
                out.iter_mut().zip(&src[yy * stride..(yy + 1) * stride]).for_each(|(o, v)| *o += v * wgt);
            }
        });
    } else {
        let radii = box_radii(sigma);
        let mut tmp = vec![0.0f32; data.len()];
        box_cols(data, &mut tmp, stride, h, radii[0], edge);
        box_cols(&tmp, data, stride, h, radii[1], edge);
        box_cols(data, &mut tmp, stride, h, radii[2], edge);
        data.copy_from_slice(&tmp);
    }
}

/// `ch` チャンネルのインターリーブ画像をガウスぼかしする。σ は画素単位。
pub fn gaussian(data: &mut [f32], w: usize, h: usize, ch: usize, sigma_x: f32, sigma_y: f32, edge: Edge) {
    if w == 0 || h == 0 {
        return;
    }
    blur_rows(data, w, ch, sigma_x, edge);
    blur_cols(data, w, h, ch, sigma_y, edge);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn energy_is_conserved_and_spread_matches_sigma() {
        for sigma in [1.5f32, 6.0, 20.0] {
            let (w, h) = (257, 129);
            let mut img = vec![0.0f32; w * h];
            img[64 * w + 128] = 1.0;
            gaussian(&mut img, w, h, 1, sigma, sigma, Edge::Zero);
            let sum: f64 = img.iter().map(|v| *v as f64).sum();
            assert!((sum - 1.0).abs() < 1e-3, "sigma {sigma}: sum {sum}");
            // x 方向の分散が σ² に近い
            let var: f64 = (0..w).map(|x| (0..h).map(|y| img[y * w + x] as f64).sum::<f64>() * (x as f64 - 128.0).powi(2)).sum();
            assert!((var.sqrt() - sigma as f64).abs() / (sigma as f64) < 0.06, "sigma {sigma}: measured {}", var.sqrt());
        }
    }

    #[test]
    fn clamp_edge_keeps_constant_image_constant() {
        let (w, h) = (40, 30);
        let mut img = vec![0.5f32; w * h * 4];
        gaussian(&mut img, w, h, 4, 8.0, 3.0, Edge::Clamp);
        assert!(img.iter().all(|v| (v - 0.5).abs() < 1e-4));
        let mut img = vec![0.5f32; w * h * 4];
        gaussian(&mut img, w, h, 4, 8.0, 8.0, Edge::Zero);
        assert!(img[0] < 0.2 && (img[(15 * w + 20) * 4] - 0.5).abs() < 0.2);
    }
}
