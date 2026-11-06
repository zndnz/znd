//! テキストレイアウト: フォント探索、シェーピング（rustybuzz）、行分割、縦書き、ルビ、字形アウトライン。
//!
//! 出力は「文字ごとの字形パス＋配置位置」。描画（塗り・線・アニメーター）は呼び出し側が行う。

use rustybuzz::ttf_parser;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use znd_core::shape::{BezPath, PathVertex};
use znd_core::text::{Justify, TextDocument};
use znd_core::Time;

/// 指定フォントに無い文字や、ファミリー未指定のときに順に試すフォント。
const FALLBACK_FAMILIES: [&str; 10] = [
    "Hiragino Sans",
    "Hiragino Kaku Gothic ProN",
    "Yu Gothic",
    "Meiryo",
    "Noto Sans CJK JP",
    "Noto Sans JP",
    "Helvetica Neue",
    "Arial",
    "DejaVu Sans",
    "Apple Symbols",
];

struct LoadedFont {
    data: Arc<Vec<u8>>,
    index: u32,
}

struct FontSystem {
    db: fontdb::Database,
    loaded: Mutex<HashMap<fontdb::ID, Arc<LoadedFont>>>,
    /// (ファミリー, ウェイト, イタリック) → 検索結果。システムフォントの検索は遅いので覚えておく。
    found: Mutex<HashMap<(String, u16, bool), Option<fontdb::ID>>>,
    /// (フォント, 字形 ID) → em 単位・Y 下向きの輪郭。可変フォント軸を使うときはキャッシュしない。
    outlines: Mutex<HashMap<(fontdb::ID, u16), Arc<Vec<BezPath>>>>,
}

fn system() -> &'static FontSystem {
    static SYS: OnceLock<FontSystem> = OnceLock::new();
    SYS.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        FontSystem { db, loaded: Mutex::default(), found: Mutex::default(), outlines: Mutex::default() }
    })
}

impl FontSystem {
    fn find(&self, family: &str, weight: u16, italic: bool) -> Option<fontdb::ID> {
        if family.is_empty() {
            return None;
        }
        let key = (family.to_string(), weight, italic);
        if let Some(hit) = self.found.lock().unwrap().get(&key) {
            return *hit;
        }
        let id = self.query(family, weight, italic);
        self.found.lock().unwrap().insert(key, id);
        id
    }

    fn query(&self, family: &str, weight: u16, italic: bool) -> Option<fontdb::ID> {
        self.db.query(&fontdb::Query {
            families: &[fontdb::Family::Name(family)],
            weight: fontdb::Weight(weight),
            stretch: fontdb::Stretch::Normal,
            style: if italic { fontdb::Style::Italic } else { fontdb::Style::Normal },
        })
    }

    fn load(&self, id: fontdb::ID) -> Option<Arc<LoadedFont>> {
        if let Some(f) = self.loaded.lock().unwrap().get(&id) {
            return Some(f.clone());
        }
        let font = self.db.with_face_data(id, |data, index| Arc::new(LoadedFont { data: Arc::new(data.to_vec()), index }))?;
        self.loaded.lock().unwrap().insert(id, font.clone());
        Some(font)
    }
}

/// システムにそのフォントファミリーがあるか（欠落フォント検出用）。
pub fn font_available(family: &str) -> bool {
    family.is_empty() || system().find(family, 400, false).is_some()
}

/// インストール済みフォントのファミリー名（重複なし・昇順）。
pub fn font_families() -> Vec<String> {
    let mut names: Vec<String> = system().db.faces().filter_map(|f| f.families.first().map(|(n, _)| n.clone())).collect();
    names.sort();
    names.dedup();
    names
}

/// インストール済みフォントのファミリー名と、その別名（日本語名など。検索用）。ファミリー名の昇順。
pub fn font_families_with_aliases() -> Vec<(String, Vec<String>)> {
    let mut map: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for f in system().db.faces() {
        let Some((name, _)) = f.families.first() else { continue };
        let aliases = map.entry(name.clone()).or_default();
        for (alias, _) in f.families.iter().skip(1) {
            if !aliases.contains(alias) {
                aliases.push(alias.clone());
            }
        }
    }
    map.into_iter().collect()
}

#[derive(Debug, Clone)]
pub struct Glyph {
    /// 本文中の文字番号（改行を除く。`znd_core::text::char_infos` と同じ数え方）。
    pub char_index: usize,
    /// 字形の輪郭（px、原点は字形原点、Y 下向き）。
    pub paths: Arc<Vec<BezPath>>,
    /// 輪郭に掛ける拡大率（em 単位 → px）。
    pub scale: f64,
    /// 字形原点のレイヤー座標。
    pub origin: [f64; 2],
    /// 文字の中心（アニメーターの回転・スケールの基準）。
    pub center: [f64; 2],
    pub advance: f64,
    pub line: usize,
    pub is_ruby: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Layout {
    pub glyphs: Vec<Glyph>,
    /// インク範囲 [左, 上, 幅, 高さ]（レイヤー座標）。
    pub bounds: [f64; 4],
    /// 指定されたが見つからなかったフォントファミリー。
    pub missing_font: Option<String>,
}

struct OutlineBuilder {
    paths: Vec<BezPath>,
    current: Vec<PathVertex>,
    /// em 単位への正規化係数（1 / units_per_em）。
    k: f64,
}

impl OutlineBuilder {
    fn pt(&self, x: f32, y: f32) -> [f64; 2] {
        [x as f64 * self.k, -(y as f64) * self.k]
    }

    fn finish(&mut self) {
        if self.current.len() > 1 {
            // 終点が始点と重なっていたら 1 頂点にまとめる
            let (first, last) = (self.current[0], self.current[self.current.len() - 1]);
            if (first.p[0] - last.p[0]).abs() < 1e-12 && (first.p[1] - last.p[1]).abs() < 1e-12 {
                self.current[0].tan_in = last.tan_in;
                self.current.pop();
            }
        }
        if self.current.len() >= 2 {
            self.paths.push(BezPath { vertices: std::mem::take(&mut self.current), closed: true });
        } else {
            self.current.clear();
        }
    }

    fn curve(&mut self, c1: [f64; 2], c2: [f64; 2], p: [f64; 2]) {
        if let Some(prev) = self.current.last_mut() {
            prev.tan_out = [c1[0] - prev.p[0], c1[1] - prev.p[1]];
        }
        self.current.push(PathVertex { p, tan_in: [c2[0] - p[0], c2[1] - p[1]], tan_out: [0.0, 0.0] });
    }
}

impl ttf_parser::OutlineBuilder for OutlineBuilder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.finish();
        let p = self.pt(x, y);
        self.current.push(PathVertex::corner(p[0], p[1]));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.pt(x, y);
        self.current.push(PathVertex::corner(p[0], p[1]));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let Some(prev) = self.current.last().map(|v| v.p) else { return };
        let (q, p) = (self.pt(x1, y1), self.pt(x, y));
        // 2 次 → 3 次
        let c1 = [prev[0] + (q[0] - prev[0]) * 2.0 / 3.0, prev[1] + (q[1] - prev[1]) * 2.0 / 3.0];
        let c2 = [p[0] + (q[0] - p[0]) * 2.0 / 3.0, p[1] + (q[1] - p[1]) * 2.0 / 3.0];
        self.curve(c1, c2, p);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (c1, c2, p) = (self.pt(x1, y1), self.pt(x2, y2), self.pt(x, y));
        self.curve(c1, c2, p);
    }
    fn close(&mut self) {
        self.finish();
    }
}

struct RunFont {
    id: fontdb::ID,
    font: Arc<LoadedFont>,
}

struct Shaper<'a> {
    fonts: Vec<RunFont>,
    doc: &'a TextDocument,
    variations: Vec<rustybuzz::Variation>,
    features: Vec<rustybuzz::Feature>,
}

struct ShapedGlyph {
    /// 行内の文字番号。
    char_in_line: usize,
    paths: Arc<Vec<BezPath>>,
    /// 行頭からの字形原点（px）。
    pos: [f64; 2],
    /// 送り（横書きは x、縦書きは y）。
    advance: f64,
}

fn tag(s: &str) -> Option<ttf_parser::Tag> {
    let b = s.as_bytes();
    (b.len() == 4).then(|| ttf_parser::Tag::from_bytes(&[b[0], b[1], b[2], b[3]]))
}

impl<'a> Shaper<'a> {
    fn new(doc: &'a TextDocument, t: Time) -> (Self, Option<String>) {
        let sys = system();
        let mut fonts = Vec::new();
        let mut missing = None;
        match sys.find(&doc.font_family, doc.weight, doc.italic) {
            Some(id) => fonts.extend(sys.load(id).map(|font| RunFont { id, font })),
            None if !doc.font_family.is_empty() => missing = Some(doc.font_family.clone()),
            None => {}
        }
        for family in FALLBACK_FAMILIES {
            if let Some(id) = sys.find(family, doc.weight, false) {
                if !fonts.iter().any(|f| f.id == id) {
                    fonts.extend(sys.load(id).map(|font| RunFont { id, font }));
                }
            }
        }
        let variations =
            doc.variations.iter().filter_map(|v| Some(rustybuzz::Variation { tag: tag(&v.tag)?, value: v.value.eval(t) as f32 })).collect();
        let features = doc
            .features
            .iter()
            .filter_map(|f| {
                let (name, value) = match f.strip_prefix('-') {
                    Some(n) => (n, 0),
                    None => (f.as_str(), 1),
                };
                Some(rustybuzz::Feature::new(tag(name)?, value, ..))
            })
            .collect();
        (Shaper { fonts, doc, variations, features }, missing)
    }

    fn face(&self, i: usize) -> Option<rustybuzz::Face<'_>> {
        let f = &self.fonts[i].font;
        let mut face = rustybuzz::Face::from_slice(&f.data, f.index)?;
        if !self.variations.is_empty() {
            face.set_variations(&self.variations);
        }
        Some(face)
    }

    /// その文字を持つ最初のフォント（どれにも無ければ先頭）。
    fn font_for<'f>(&'f self, faces: &[std::cell::OnceCell<Option<rustybuzz::Face<'f>>>], ch: char) -> usize {
        // フォールバック用のフォントは、実際に必要になったときに初めて解析する（毎回全部を解析すると重い）
        (0..faces.len())
            .find(|i| faces[*i].get_or_init(|| self.face(*i)).as_ref().is_some_and(|f| f.glyph_index(ch).is_some()))
            .unwrap_or(0)
    }

    fn outline(&self, font: usize, face: &rustybuzz::Face<'_>, glyph: u16) -> Arc<Vec<BezPath>> {
        let sys = system();
        let key = (self.fonts[font].id, glyph);
        let cacheable = self.variations.is_empty();
        if cacheable {
            if let Some(p) = sys.outlines.lock().unwrap().get(&key) {
                return p.clone();
            }
        }
        let mut b = OutlineBuilder { paths: Vec::new(), current: Vec::new(), k: 1.0 / face.units_per_em() as f64 };
        face.outline_glyph(ttf_parser::GlyphId(glyph), &mut b);
        b.finish();
        let paths = Arc::new(b.paths);
        if cacheable {
            sys.outlines.lock().unwrap().insert(key, paths.clone());
        }
        paths
    }

    /// 1 行をシェーピングする。`tracking` は文字ごとの追加トラッキング（1/1000 em、行内の文字番号で引く）。
    fn shape_line(&self, line: &str, size: f64, tracking: &dyn Fn(usize) -> f64) -> (Vec<ShapedGlyph>, f64) {
        let faces: Vec<std::cell::OnceCell<Option<rustybuzz::Face<'_>>>> =
            (0..self.fonts.len()).map(|_| std::cell::OnceCell::new()).collect();
        let mut out = Vec::new();
        let mut pen = 0.0f64;
        if faces.is_empty() {
            return (out, pen);
        }
        // フォントが切り替わる位置で分割し、それぞれをシェーピングする
        let chars: Vec<(usize, char)> = line.char_indices().collect();
        let mut start = 0;
        while start < chars.len() {
            let font = self.font_for(&faces, chars[start].1);
            let mut end = start + 1;
            while end < chars.len() && self.font_for(&faces, chars[end].1) == font {
                end += 1;
            }
            let byte_start = chars[start].0;
            let byte_end = chars.get(end).map(|c| c.0).unwrap_or(line.len());
            if let Some(face) = faces[font].get_or_init(|| self.face(font)) {
                let mut buf = rustybuzz::UnicodeBuffer::new();
                buf.push_str(&line[byte_start..byte_end]);
                if self.doc.vertical {
                    buf.set_direction(rustybuzz::Direction::TopToBottom);
                }
                let shaped = rustybuzz::shape(face, &self.features, buf);
                let k = size / face.units_per_em() as f64;
                for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
                    let byte = byte_start + info.cluster as usize;
                    let char_in_line = chars.partition_point(|c| c.0 < byte).min(chars.len() - 1);
                    let track = (self.doc.tracking + tracking(char_in_line)) / 1000.0 * size;
                    let paths = self.outline(font, face, info.glyph_id as u16);
                    let (ox, oy) = (pos.x_offset as f64 * k, -(pos.y_offset as f64) * k);
                    if self.doc.vertical {
                        let adv = -(pos.y_advance as f64) * k + track;
                        out.push(ShapedGlyph { char_in_line, paths, pos: [ox, pen + oy], advance: adv });
                        pen += adv;
                    } else {
                        let adv = pos.x_advance as f64 * k + track;
                        out.push(ShapedGlyph { char_in_line, paths, pos: [pen + ox, oy], advance: adv });
                        pen += adv;
                    }
                }
            }
            start = end;
        }
        (out, pen)
    }
}

fn is_cjk(ch: char) -> bool {
    ch as u32 >= 0x2E80
}

/// 行頭に来てはいけない文字（行頭禁則）。
fn no_line_start(ch: char) -> bool {
    "、。，．・：；？！゛゜ヽヾゝゞ々ー」』）】〕］｝〉》〟’”ぁぃぅぇぉっゃゅょゎァィゥェォッャュョヮ,.!?)]}".contains(ch)
}

/// 行末に来てはいけない文字（行末禁則）。
fn no_line_end(ch: char) -> bool {
    "「『（【〔［｛〈《〝‘“([{".contains(ch)
}

/// 段落テキストの自動折り返し。各行を (文字列, 本文での開始文字番号) で返す。
fn wrap(shaper: &Shaper, paragraph: &str, first_char: usize, size: f64, limit: f64) -> Vec<(String, usize)> {
    let chars: Vec<char> = paragraph.chars().collect();
    let mut lines = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        // 収まる最長の位置を探す（1 文字は必ず入れる）
        let mut end = start + 1;
        let mut best = None;
        while end <= chars.len() {
            let s: String = chars[start..end].iter().collect();
            let (_, width) = shaper.shape_line(s.trim_end(), size, &|_| 0.0);
            if width > limit && end > start + 1 {
                break;
            }
            // end が折り返し可能な位置か
            let breakable = end == chars.len()
                || chars[end - 1].is_whitespace()
                || ((is_cjk(chars[end - 1]) || is_cjk(chars[end])) && !no_line_start(chars[end]) && !no_line_end(chars[end - 1]));
            if breakable {
                best = Some(end);
            }
            end += 1;
        }
        let cut = best.unwrap_or((end - 1).max(start + 1)).min(chars.len());
        lines.push((chars[start..cut].iter().collect::<String>(), first_char + start));
        start = cut;
    }
    if lines.is_empty() {
        lines.push((String::new(), first_char));
    }
    lines
}

fn path_bounds(paths: &[BezPath], scale: f64, origin: [f64; 2], b: &mut [f64; 4]) {
    for v in paths.iter().flat_map(|p| &p.vertices) {
        let (x, y) = (origin[0] + v.p[0] * scale, origin[1] + v.p[1] * scale);
        *b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
    }
}

/// テキストをレイアウトする。
///
/// - `text`: 実際に組む文字列（文字オフセットアニメーター適用後など）。`None` なら `doc.text`。
/// - `tracking`: 文字ごとの追加トラッキング（1/1000 em）。本文の文字番号で引く。
pub fn layout(doc: &TextDocument, text: Option<&str>, tracking: &dyn Fn(usize) -> f64, t: Time) -> Layout {
    let text = text.unwrap_or(&doc.text);
    let (shaper, missing_font) = Shaper::new(doc, t);
    let size = doc.size;
    let leading = doc.leading.unwrap_or(size * 1.2);

    // 行に分ける（明示的な改行 → 段落テキストなら自動折り返し）
    let mut lines: Vec<(String, usize)> = Vec::new();
    let mut char_base = 0;
    for para in text.split('\n') {
        match doc.box_size {
            Some(b) => {
                let limit = if doc.vertical { b[1] } else { b[0] };
                lines.extend(wrap(&shaper, para, char_base, size, limit));
            }
            None => lines.push((para.to_string(), char_base)),
        }
        char_base += para.chars().count();
    }

    let mut glyphs = Vec::new();
    for (li, (line, first)) in lines.iter().enumerate() {
        let (shaped, extent) = shaper.shape_line(line, size, &|i| tracking(first + i));
        // 揃え。ポイントテキストは原点基準、段落テキストはボックス基準。
        let box_len = doc.box_size.map(|b| if doc.vertical { b[1] } else { b[0] });
        let shift = match (doc.justify, box_len) {
            (Justify::Left, _) => 0.0,
            (Justify::Center, None) => -extent / 2.0,
            (Justify::Right, None) => -extent,
            (Justify::Center, Some(l)) => (l - extent) / 2.0,
            (Justify::Right, Some(l)) => l - extent,
        };
        for g in shaped {
            let (origin, center) = if doc.vertical {
                // 縦書き: 行は右から左へ進む。字形原点は縦組み原点（上辺中央）
                let x = -(li as f64) * leading;
                let o = [x + g.pos[0], shift + g.pos[1]];
                (o, [x, shift + g.pos[1] + g.advance / 2.0])
            } else {
                let y = li as f64 * leading;
                let o = [shift + g.pos[0], y + g.pos[1]];
                (o, [o[0] + g.advance / 2.0, y - size * 0.35])
            };
            glyphs.push(Glyph {
                char_index: first + g.char_in_line,
                paths: g.paths,
                scale: size,
                origin,
                center,
                advance: g.advance,
                line: li,
                is_ruby: false,
            });
        }
    }

    // ルビ: 親文字の範囲の中央に、半分のサイズで組む
    let ruby_size = size * 0.5;
    for ruby in &doc.ruby {
        let base: Vec<&Glyph> = glyphs.iter().filter(|g| !g.is_ruby && g.char_index >= ruby.start && g.char_index < ruby.end).collect();
        let (Some(first), Some(last)) = (base.first(), base.last()) else { continue };
        if first.line != last.line {
            continue;
        }
        let mut rdoc = doc.clone();
        rdoc.tracking = 0.0;
        let (rshaper, _) = Shaper::new(&rdoc, t);
        let (shaped, extent) = rshaper.shape_line(&ruby.text, ruby_size, &|_| 0.0);
        let (char_index, line) = (first.char_index, first.line);
        let new: Vec<Glyph> = if doc.vertical {
            let (top, bottom) = (first.center[1] - first.advance / 2.0, last.center[1] + last.advance / 2.0);
            let y0 = (top + bottom) / 2.0 - extent / 2.0;
            let x = first.center[0] + size * 0.5 + ruby_size * 0.5;
            shaped
                .into_iter()
                .map(|g| Glyph {
                    char_index,
                    paths: g.paths,
                    scale: ruby_size,
                    origin: [x + g.pos[0], y0 + g.pos[1]],
                    center: first.center,
                    advance: g.advance,
                    line,
                    is_ruby: true,
                })
                .collect()
        } else {
            let (left, right) = (first.origin[0], last.origin[0] + last.advance);
            let x0 = (left + right) / 2.0 - extent / 2.0;
            let y = first.origin[1] - size * 0.92;
            shaped
                .into_iter()
                .map(|g| Glyph {
                    char_index,
                    paths: g.paths,
                    scale: ruby_size,
                    origin: [x0 + g.pos[0], y + g.pos[1]],
                    center: first.center,
                    advance: g.advance,
                    line,
                    is_ruby: true,
                })
                .collect()
        };
        glyphs.extend(new);
    }

    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for g in &glyphs {
        path_bounds(&g.paths, g.scale, g.origin, &mut b);
    }
    let bounds = if b[0] > b[2] { [0.0; 4] } else { [b[0], b[1], b[2] - b[0], b[3] - b[1]] };
    Layout { glyphs, bounds, missing_font }
}

/// 字形の輪郭をレイヤー座標のパスにする（アウトライン化・パーティクルのエミッター等）。
pub fn glyph_paths(g: &Glyph) -> Vec<BezPath> {
    g.paths
        .iter()
        .map(|p| BezPath {
            vertices: p
                .vertices
                .iter()
                .map(|v| PathVertex {
                    p: [g.origin[0] + v.p[0] * g.scale, g.origin[1] + v.p[1] * g.scale],
                    tan_in: [v.tan_in[0] * g.scale, v.tan_in[1] * g.scale],
                    tan_out: [v.tan_out[0] * g.scale, v.tan_out[1] * g.scale],
                })
                .collect(),
            closed: p.closed,
        })
        .collect()
}

/// テキストをシェイプ（パス群）に変換する。
pub fn outlines(doc: &TextDocument, t: Time) -> Vec<BezPath> {
    layout(doc, None, &|_| 0.0, t).glyphs.iter().flat_map(glyph_paths).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use znd_core::text::Ruby;

    fn lay(doc: &TextDocument) -> Layout {
        layout(doc, None, &|_| 0.0, Time::ZERO)
    }

    /// フォントが 1 つも無い環境（最小構成の CI など）ではテストを飛ばす。
    fn have_fonts() -> bool {
        let ok = !lay(&TextDocument::new("A", 100.0)).glyphs.is_empty();
        if !ok {
            eprintln!("フォントが見つからないためスキップ");
        }
        ok
    }

    #[test]
    fn horizontal_layout_advances_and_scales() {
        if !have_fonts() {
            return;
        }
        let l = lay(&TextDocument::new("ABC", 100.0));
        assert_eq!(l.glyphs.len(), 3);
        assert!(l.glyphs[0].origin[0] < l.glyphs[1].origin[0] && l.glyphs[1].origin[0] < l.glyphs[2].origin[0]);
        assert_eq!(l.glyphs.iter().map(|g| g.char_index).collect::<Vec<_>>(), vec![0, 1, 2]);
        // インク範囲はベースラインより上、高さはサイズの 6〜8 割程度
        assert!(l.bounds[1] < 0.0 && l.bounds[3] > 50.0 && l.bounds[3] < 90.0, "{:?}", l.bounds);
        let big = lay(&TextDocument::new("ABC", 200.0));
        assert!((big.bounds[2] / l.bounds[2] - 2.0).abs() < 0.01);
    }

    #[test]
    fn japanese_multiline_and_justify() {
        if !have_fonts() {
            return;
        }
        let mut doc = TextDocument::new("こんにちは\n世界", 60.0);
        let l = lay(&doc);
        assert_eq!(l.glyphs.len(), 7);
        assert_eq!(l.glyphs[5].line, 1);
        assert_eq!(l.glyphs[5].char_index, 5);
        assert!((l.glyphs[5].origin[1] - 72.0).abs() < 1e-9);
        // 全角文字の送りはほぼ 1em
        assert!((l.glyphs[0].advance - 60.0).abs() < 3.0, "{}", l.glyphs[0].advance);
        doc.justify = Justify::Center;
        let c = lay(&doc);
        assert!((c.bounds[0] + c.bounds[2] / 2.0).abs() < 4.0, "{:?}", c.bounds);
    }

    #[test]
    fn tracking_spreads_glyphs() {
        if !have_fonts() {
            return;
        }
        let mut doc = TextDocument::new("AAAA", 100.0);
        let base = lay(&doc);
        doc.tracking = 200.0;
        let wide = lay(&doc);
        let step = |l: &Layout| l.glyphs[1].origin[0] - l.glyphs[0].origin[0];
        assert!((step(&wide) - step(&base) - 20.0).abs() < 1e-6);
        // 文字ごとのトラッキング（アニメーター用）
        doc.tracking = 0.0;
        let per = layout(&doc, None, &|i| if i == 0 { 500.0 } else { 0.0 }, Time::ZERO);
        assert!((step(&per) - step(&base) - 50.0).abs() < 1e-6);
    }

    #[test]
    fn vertical_writing_flows_downwards_and_lines_go_left() {
        if !have_fonts() {
            return;
        }
        let mut doc = TextDocument::new("縦書き\n二行", 50.0);
        doc.vertical = true;
        let l = lay(&doc);
        assert_eq!(l.glyphs.len(), 5);
        assert!(l.glyphs[1].center[1] > l.glyphs[0].center[1] + 30.0);
        assert!((l.glyphs[1].center[0] - l.glyphs[0].center[0]).abs() < 1e-9);
        assert!(l.glyphs[3].center[0] < l.glyphs[0].center[0] - 50.0);
        assert!(l.bounds[3] > l.bounds[2], "縦長になるはず: {:?}", l.bounds);
    }

    #[test]
    fn paragraph_text_wraps_with_kinsoku() {
        if !have_fonts() {
            return;
        }
        let mut doc = TextDocument::new("あいうえお、かきくけこ", 40.0);
        doc.box_size = Some([205.0, 400.0]);
        let l = lay(&doc);
        // 5 文字で折り返すと「、」が行頭に来てしまうので、1 文字手前で折り返す
        let second_line_first = l.glyphs.iter().find(|g| g.line == 1).unwrap();
        assert_eq!(second_line_first.char_index, 4, "行頭禁則");
        assert!(l.glyphs.iter().all(|g| g.origin[0] < 206.0));
        let lines = l.glyphs.iter().map(|g| g.line).max().unwrap() + 1;
        assert!(lines >= 3);
    }

    #[test]
    fn ruby_is_centered_above_base() {
        if !have_fonts() {
            return;
        }
        let mut doc = TextDocument::new("漢字です", 80.0);
        doc.ruby = vec![Ruby { start: 0, end: 2, text: "かんじ".into() }];
        let l = lay(&doc);
        let ruby: Vec<&Glyph> = l.glyphs.iter().filter(|g| g.is_ruby).collect();
        assert_eq!(ruby.len(), 3);
        assert!(ruby.iter().all(|g| g.scale == 40.0 && g.origin[1] < -60.0 && g.char_index == 0));
        let ruby_mid = (ruby[0].origin[0] + ruby[2].origin[0] + ruby[2].advance) / 2.0;
        let base_mid = (l.glyphs[0].origin[0] + l.glyphs[1].origin[0] + l.glyphs[1].advance) / 2.0;
        assert!((ruby_mid - base_mid).abs() < 1.0);
    }

    #[test]
    fn missing_font_is_reported_and_falls_back() {
        if !have_fonts() {
            return;
        }
        let mut doc = TextDocument::new("abc", 50.0);
        doc.font_family = "このフォントは存在しない".into();
        let l = lay(&doc);
        assert_eq!(l.missing_font.as_deref(), Some("このフォントは存在しない"));
        assert_eq!(l.glyphs.len(), 3);
        assert!(!font_available("このフォントは存在しない"));
        assert!(font_available(""));
        assert!(!outlines(&doc, Time::ZERO).is_empty());
    }
}
