//! 画像バッファ。すべて RGBA・リニアライト・プリマルチプライド・f32。

use std::cell::RefCell;

#[derive(Debug, Clone, PartialEq)]
pub struct FrameBuf {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl FrameBuf {
    pub fn new(width: usize, height: usize) -> Self {
        FrameBuf { width, height, data: vec![0.0; width * height * 4] }
    }

    pub fn pixel(&self, x: usize, y: usize) -> [f32; 4] {
        let i = (y * self.width + x) * 4;
        [self.data[i], self.data[i + 1], self.data[i + 2], self.data[i + 3]]
    }

    pub fn set_pixel(&mut self, x: usize, y: usize, p: [f32; 4]) {
        let i = (y * self.width + x) * 4;
        self.data[i..i + 4].copy_from_slice(&p);
    }
}

thread_local! {
    /// 使い終わったタイルのバッファ。レイヤーごとに巨大な確保・解放を繰り返すと
    /// ページフォールトが支配的になり、スレッドを増やしても速くならないため再利用する。
    static POOL: RefCell<Vec<Vec<f32>>> = const { RefCell::new(Vec::new()) };
}

/// スレッドごとに保持するバッファ数（レイヤー本体＋トラックマット）。
const POOL_SIZE: usize = 2;

/// ゼロで埋めた長さ `len` のバッファを、可能なら再利用して返す。
pub(crate) fn take_zeroed(len: usize) -> Vec<f32> {
    let mut v = POOL.with(|p| p.borrow_mut().pop()).unwrap_or_default();
    v.clear();
    v.resize(len, 0.0);
    v
}

pub(crate) fn recycle(v: Vec<f32>) {
    POOL.with(|p| {
        let mut pool = p.borrow_mut();
        if pool.len() < POOL_SIZE {
            pool.push(v);
        }
    });
}

/// 出力キャンバス上の矩形領域だけを持つレイヤー画像。
#[derive(Debug)]
pub struct Tile {
    pub x0: usize,
    pub y0: usize,
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl Tile {
    /// キャンバス座標での画素値。タイル外は透明。
    pub fn pixel(&self, x: usize, y: usize) -> [f32; 4] {
        if x < self.x0 || y < self.y0 || x >= self.x0 + self.w || y >= self.y0 + self.h {
            return [0.0; 4];
        }
        let i = ((y - self.y0) * self.w + (x - self.x0)) * 4;
        [self.data[i], self.data[i + 1], self.data[i + 2], self.data[i + 3]]
    }
}

impl Drop for Tile {
    fn drop(&mut self) {
        recycle(std::mem::take(&mut self.data));
    }
}
