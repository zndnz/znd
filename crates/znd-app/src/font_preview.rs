//! フォントの選択欄に出す見本。選んでいるテキストの本文（無ければ既定の見本）を各フォントで描いた画像を、
//! 別スレッドで作って使い回す。一覧を開いたときに見えている行の分だけ作る。

use eframe::egui::{self, ColorImage, TextureHandle};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Condvar, Mutex};
use znd_core::shape::FillRule;
use znd_core::text::TextDocument;
use znd_core::Time;
use znd_render::path::{fill_coverage, flatten, Polyline};

/// 本文が空のときの見本（かな・漢字・英字を混ぜて、和文フォントか欧文フォントかが分かるように）。
pub(crate) const DEFAULT_SAMPLE: &str = "あア永 Aa";

type Done = Arc<Mutex<Vec<(String, String, Option<ColorImage>)>>>;
/// 頼まれた見本（後から頼まれたものを先に作る。検索で絞り込んだ行がすぐ埋まるように）。
type Queue = Arc<(Mutex<Vec<(String, String, f32)>>, Condvar)>;

#[derive(Default)]
pub(crate) struct FontPreviews {
    sample: String,
    textures: HashMap<String, Option<TextureHandle>>,
    pending: HashSet<String>,
    queue: Option<Queue>,
    done: Done,
}

/// 見本にする文字列（本文の先頭 1 行・最大 16 文字）。
pub(crate) fn sample_of(text: &str) -> String {
    let line: String = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(16).collect();
    if line.is_empty() {
        DEFAULT_SAMPLE.to_string()
    } else {
        line
    }
}

impl FontPreviews {
    /// `family` で `sample` を描いた画像（高さ `height` 画素）。まだできていなければ作るよう頼んで None を返す。
    pub fn get(&mut self, ctx: &egui::Context, family: &str, sample: &str, height: f32) -> Option<TextureHandle> {
        if sample != self.sample {
            self.sample = sample.to_string();
            self.textures.clear();
            self.pending.clear();
            if let Some(q) = &self.queue {
                q.0.lock().unwrap_or_else(|e| e.into_inner()).clear();
            }
        }
        self.collect(ctx);
        if let Some(t) = self.textures.get(family) {
            return t.clone();
        }
        if self.pending.insert(family.to_string()) {
            let q = self.queue.get_or_insert_with(|| spawn(ctx.clone(), self.done.clone()));
            q.0.lock().unwrap_or_else(|e| e.into_inner()).push((family.to_string(), sample.to_string(), height));
            q.1.notify_one();
        }
        None
    }

    fn collect(&mut self, ctx: &egui::Context) {
        let done: Vec<_> = std::mem::take(&mut *self.done.lock().unwrap_or_else(|e| e.into_inner()));
        for (family, sample, image) in done {
            if sample != self.sample {
                continue;
            }
            self.pending.remove(&family);
            let tex = image.map(|img| ctx.load_texture(format!("font-preview:{family}"), img, egui::TextureOptions::LINEAR));
            self.textures.insert(family, tex);
        }
    }
}

fn spawn(ctx: egui::Context, done: Done) -> Queue {
    let queue = Queue::default();
    let q = queue.clone();
    let _ = std::thread::Builder::new().name("znd-font-preview".into()).spawn(move || loop {
        let (family, sample, height) = {
            let mut list = q.0.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                match list.pop() {
                    Some(job) => break job,
                    None => list = q.1.wait(list).unwrap_or_else(|e| e.into_inner()),
                }
            }
        };
        {
            let image = std::panic::catch_unwind(|| rasterize(&family, &sample, height)).ok().flatten();
            done.lock().unwrap_or_else(|e| e.into_inner()).push((family, sample, image));
            ctx.request_repaint();
        }
    });
    queue
}

/// 見本を白の文字で描く（背景は透明）。字形が 1 つも無ければ None。
pub(crate) fn rasterize(family: &str, sample: &str, height: f32) -> Option<ColorImage> {
    let h = height.round().clamp(8.0, 128.0) as usize;
    let mut doc = TextDocument::new(sample, h as f64 * 0.7);
    doc.font_family = family.to_string();
    let lines: Vec<Polyline> = znd_text::outlines(&doc, Time::ZERO).iter().map(|p| flatten(p, 0.15)).filter(|l| l.pts.len() > 1).collect();
    let pts = lines.iter().flat_map(|l| l.pts.iter());
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in pts {
        (x0, y0, x1, y1) = (x0.min(p[0]), y0.min(p[1]), x1.max(p[0]), y1.max(p[1]));
    }
    if lines.is_empty() || x1 <= x0 {
        return None;
    }
    let w = ((x1 - x0).ceil() as usize + 4).min(h * 16);
    let (dx, dy) = (2.0 - x0, h as f64 / 2.0 - (y0 + y1) / 2.0);
    let moved: Vec<Polyline> =
        lines.into_iter().map(|l| Polyline { pts: l.pts.iter().map(|p| [p[0] + dx, p[1] + dy]).collect(), closed: l.closed }).collect();
    let alpha = fill_coverage(&moved, FillRule::NonZero, w, h).to_dense(w, h);
    let pixels = alpha.iter().map(|a| egui::Color32::from_white_alpha((a.clamp(0.0, 1.0) * 255.0) as u8)).collect();
    Some(ColorImage { size: [w, h], pixels })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_uses_first_line_of_text() {
        assert_eq!(sample_of("\n  こんにちは世界  \n二行目"), "こんにちは世界");
        assert_eq!(sample_of("   "), DEFAULT_SAMPLE);
        assert_eq!(sample_of(&"あ".repeat(40)).chars().count(), 16);
    }

    #[test]
    fn rasterizes_default_font() {
        // フォントが 1 つも無い環境では作れないので、そのときは何も確かめない
        if let Some(img) = rasterize("", "Aa", 24.0) {
            assert_eq!(img.size[1], 24);
            assert!(img.pixels.iter().any(|p| p.a() > 128));
        }
    }
}
