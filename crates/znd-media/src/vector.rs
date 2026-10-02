//! ベクター素材（SVG・PDF・PDF 互換で保存された AI）をシェイプレイヤーとして読み込む。
//!
//! 図形はラスタライズせずベジェパスのまま持つので、拡大しても滲まない。変換行列は座標に焼き込む。
//! 再現できない要素（画像・パターン・クリッピング・フィルター等）は読み飛ばし、`warnings` に列挙する。

use crate::MediaError;
use std::collections::BTreeSet;
use std::path::Path;
use znd_core::shape::{BezPath, FillRule, Gradient, LineCap, LineJoin, PathVertex, ShapeItem, ShapeTransform};
use znd_core::{Affine, Layer, LayerKind, Property, Time, Transform};

#[derive(Debug, Clone)]
pub struct VectorImport {
    pub name: String,
    /// アートボード（ページ）の大きさ。px（PDF は 1pt = 1px）。
    pub width: f64,
    pub height: f64,
    pub items: Vec<ShapeItem>,
    pub path_count: usize,
    pub warnings: Vec<String>,
}

impl VectorImport {
    /// アートボードの中心をコンポの中央に置いたシェイプレイヤーを作る。
    pub fn into_layer(self, id: &str, comp_w: f64, comp_h: f64, duration: Time) -> Layer {
        let tr = Transform::centered(self.width, self.height, comp_w, comp_h);
        Layer::new(id, self.name, LayerKind::Shape { items: self.items }, tr, duration)
    }
}

pub fn is_vector(path: &Path) -> bool {
    matches!(ext(path).as_str(), "svg" | "svgz" | "pdf" | "ai")
}

fn ext(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

pub fn import_vector(path: &Path) -> Result<VectorImport, MediaError> {
    let err = |message: String| MediaError::File { path: path.display().to_string(), message };
    let data = std::fs::read(path).map_err(|e| err(e.to_string()))?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut out = match ext(path).as_str() {
        "svg" | "svgz" => import_svg(&data).map_err(err)?,
        "pdf" => import_pdf(&data).map_err(err)?,
        "ai" => {
            // Illustrator の「PDF 互換ファイルを作成」で保存された AI は中身が PDF
            if !data.windows(5).take(1024).any(|w| w == b"%PDF-") {
                return Err(err("PDF 互換で保存されていない AI ファイルです。Illustrator で「PDF 互換ファイルを作成」を有効にして保存し直すか、SVG で書き出してください".into()));
            }
            import_pdf(&data).map_err(err)?
        }
        other => return Err(err(format!("未対応のベクター形式です: .{other}"))),
    };
    out.name = name;
    Ok(out)
}

// ---------- 共通: パスの組み立て ----------

/// 1 つの描画オブジェクト（複数のサブパス＋塗り＋線）。
#[derive(Default)]
struct Builder {
    subpaths: Vec<BezPath>,
    current: Option<BezPath>,
}

impl Builder {
    fn move_to(&mut self, p: [f64; 2]) {
        self.finish_subpath();
        self.current = Some(BezPath { vertices: vec![PathVertex::corner(p[0], p[1])], closed: false });
    }
    fn last(&self) -> Option<[f64; 2]> {
        self.current.as_ref().and_then(|c| c.vertices.last()).map(|v| v.p)
    }
    fn line_to(&mut self, p: [f64; 2]) {
        match &mut self.current {
            Some(c) => c.vertices.push(PathVertex::corner(p[0], p[1])),
            None => self.move_to(p),
        }
    }
    fn cubic_to(&mut self, c1: [f64; 2], c2: [f64; 2], p: [f64; 2]) {
        let Some(cur) = &mut self.current else { return self.move_to(p) };
        let last = cur.vertices.last_mut().expect("subpath has a start point");
        last.tan_out = [c1[0] - last.p[0], c1[1] - last.p[1]];
        cur.vertices.push(PathVertex { p, tan_in: [c2[0] - p[0], c2[1] - p[1]], tan_out: [0.0; 2] });
    }
    fn quad_to(&mut self, c: [f64; 2], p: [f64; 2]) {
        let Some(a) = self.last() else { return self.move_to(p) };
        let c1 = [a[0] + (c[0] - a[0]) * 2.0 / 3.0, a[1] + (c[1] - a[1]) * 2.0 / 3.0];
        let c2 = [p[0] + (c[0] - p[0]) * 2.0 / 3.0, p[1] + (c[1] - p[1]) * 2.0 / 3.0];
        self.cubic_to(c1, c2, p);
    }
    fn close(&mut self) {
        if let Some(mut c) = self.current.take() {
            // 終点が始点に重なっていれば 1 つにまとめる（接線は引き継ぐ）
            if c.vertices.len() > 1 {
                let (first, last) = (c.vertices[0], c.vertices[c.vertices.len() - 1]);
                if (first.p[0] - last.p[0]).abs() < 1e-9 && (first.p[1] - last.p[1]).abs() < 1e-9 {
                    c.vertices[0].tan_in = last.tan_in;
                    c.vertices.pop();
                }
            }
            c.closed = true;
            let start = c.vertices[0].p;
            self.subpaths.push(c);
            // 閉じた後に続けて描く場合の始点
            self.current = Some(BezPath { vertices: vec![PathVertex::corner(start[0], start[1])], closed: false });
        }
    }
    fn finish_subpath(&mut self) {
        if let Some(c) = self.current.take() {
            if c.vertices.len() > 1 {
                self.subpaths.push(c);
            }
        }
    }
    fn take(&mut self) -> Vec<BezPath> {
        self.finish_subpath();
        std::mem::take(&mut self.subpaths)
    }
}

struct FillStyle {
    color: [f64; 4],
    opacity: f64,
    rule: FillRule,
    gradient: Option<Gradient>,
}

struct StrokeStyle {
    color: [f64; 4],
    opacity: f64,
    width: f64,
    cap: LineCap,
    join: LineJoin,
    miter_limit: f64,
    dashes: Vec<f64>,
    dash_offset: f64,
    gradient: Option<Gradient>,
}

fn object(
    name: String,
    paths: Vec<BezPath>,
    fill: Option<FillStyle>,
    stroke: Option<StrokeStyle>,
    stroke_first: bool,
) -> Option<ShapeItem> {
    if paths.is_empty() || (fill.is_none() && stroke.is_none()) {
        return None;
    }
    let mut items: Vec<ShapeItem> = paths.into_iter().map(|p| ShapeItem::Path { path: Property::constant(p) }).collect();
    let fill = fill.map(|f| ShapeItem::Fill {
        color: Property::constant(f.color),
        opacity: Property::constant(f.opacity * 100.0),
        rule: f.rule,
        gradient: f.gradient,
    });
    let stroke = stroke.map(|s| ShapeItem::Stroke {
        color: Property::constant(s.color),
        opacity: Property::constant(s.opacity * 100.0),
        width: Property::constant(s.width),
        cap: s.cap,
        join: s.join,
        miter_limit: s.miter_limit,
        dashes: s.dashes,
        dash_offset: Property::constant(s.dash_offset),
        gradient: s.gradient,
    });
    // シェイプレイヤーは後に書いた塗り・線ほど上に描く
    if stroke_first {
        items.extend(stroke);
        items.extend(fill);
    } else {
        items.extend(fill);
        items.extend(stroke);
    }
    Some(ShapeItem::Group { name, items, transform: ShapeTransform::default() })
}

fn apply(m: &Affine, p: [f64; 2]) -> [f64; 2] {
    [m.a * p[0] + m.c * p[1] + m.tx, m.b * p[0] + m.d * p[1] + m.ty]
}

fn scale_of(m: &Affine) -> f64 {
    (m.a * m.d - m.b * m.c).abs().sqrt()
}

// ---------- SVG ----------

/// SVG のグラデーションは表示値（sRGB 符号化後）で補間される。レンダラーはリニアライトで補間するので、
/// 中間の色を表示値で補間して分割点として足し、見た目を合わせる。
fn densify_stops(stops: &[(f64, [f64; 4])]) -> Vec<(f64, [f64; 4])> {
    const STEPS: usize = 8;
    let mut out = Vec::new();
    for w in stops.windows(2) {
        let ((p0, c0), (p1, c1)) = (w[0], w[1]);
        for i in 0..STEPS {
            let t = i as f64 / STEPS as f64;
            out.push((p0 + (p1 - p0) * t, [0, 1, 2, 3].map(|k| c0[k] + (c1[k] - c0[k]) * t)));
        }
    }
    out.extend(stops.last().copied());
    out
}

fn import_svg(data: &[u8]) -> Result<VectorImport, String> {
    let mut opt = usvg::Options::default();
    opt.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_data(data, &opt).map_err(|e| format!("SVG を解析できません: {e}"))?;
    let mut cx = SvgCx { items: Vec::new(), warnings: BTreeSet::new(), count: 0 };
    cx.group(tree.root(), 1.0);
    let size = tree.size();
    // 先に書かれた図形ほど下。シェイプレイヤーは先頭が最前面なので逆順にする
    cx.items.reverse();
    Ok(VectorImport {
        name: String::new(),
        width: size.width() as f64,
        height: size.height() as f64,
        items: cx.items,
        path_count: cx.count,
        warnings: cx.warnings.into_iter().collect(),
    })
}

struct SvgCx {
    items: Vec<ShapeItem>,
    warnings: BTreeSet<String>,
    count: usize,
}

fn svg_affine(t: usvg::Transform) -> Affine {
    Affine { a: t.sx as f64, b: t.ky as f64, c: t.kx as f64, d: t.sy as f64, tx: t.tx as f64, ty: t.ty as f64 }
}

impl SvgCx {
    fn group(&mut self, g: &usvg::Group, opacity: f64) {
        let opacity = opacity * g.opacity().get() as f64;
        if g.opacity().get() < 1.0 && g.children().len() > 1 {
            self.warnings.insert("グループの不透明度は各図形に掛けています（図形が重なる部分は元より濃くなります）".into());
        }
        if g.clip_path().is_some() {
            self.warnings.insert("クリッピングパスは無視しました".into());
        }
        if g.mask().is_some() {
            self.warnings.insert("マスクは無視しました".into());
        }
        if !g.filters().is_empty() {
            self.warnings.insert("フィルター効果は無視しました".into());
        }
        if g.blend_mode() != usvg::BlendMode::Normal {
            self.warnings.insert("図形ごとの描画モードは無視しました（通常で合成）".into());
        }
        for node in g.children() {
            match node {
                usvg::Node::Group(g) => self.group(g, opacity),
                usvg::Node::Path(p) => self.path(p, opacity),
                usvg::Node::Text(t) => {
                    self.warnings.insert("テキストはアウトライン（パス）に変換しました".into());
                    self.group(t.flattened(), opacity);
                }
                usvg::Node::Image(_) => {
                    self.warnings.insert("埋め込み画像は読み込んでいません".into());
                }
            }
        }
    }

    fn paint(&mut self, paint: &usvg::Paint, m: &Affine) -> Option<([f64; 4], Option<Gradient>)> {
        let stops = |stops: &[usvg::Stop]| -> Vec<(f64, [f64; 4])> {
            let raw: Vec<(f64, [f64; 4])> = stops
                .iter()
                .map(|s| {
                    let c = s.color();
                    (
                        s.offset().get() as f64,
                        [c.red as f64 / 255.0, c.green as f64 / 255.0, c.blue as f64 / 255.0, s.opacity().get() as f64],
                    )
                })
                .collect();
            densify_stops(&raw)
        };
        match paint {
            usvg::Paint::Color(c) => Some(([c.red as f64 / 255.0, c.green as f64 / 255.0, c.blue as f64 / 255.0, 1.0], None)),
            usvg::Paint::LinearGradient(g) => {
                let gm = m.then_after(&svg_affine(g.transform()));
                let grad = Gradient {
                    radial: false,
                    start: Property::constant(apply(&gm, [g.x1() as f64, g.y1() as f64])),
                    end: Property::constant(apply(&gm, [g.x2() as f64, g.y2() as f64])),
                    stops: stops(g.stops()),
                };
                Some(([1.0; 4], Some(grad)))
            }
            usvg::Paint::RadialGradient(g) => {
                let gm = m.then_after(&svg_affine(g.transform()));
                if (g.fx() - g.cx()).abs() > 1e-3 || (g.fy() - g.cy()).abs() > 1e-3 || (gm.a * gm.c + gm.b * gm.d).abs() > 1e-6 {
                    self.warnings.insert("焦点がずれた／楕円形の円形グラデーションは、中心が同じ正円として近似しました".into());
                }
                let c = [g.cx() as f64, g.cy() as f64];
                let grad = Gradient {
                    radial: true,
                    start: Property::constant(apply(&gm, c)),
                    end: Property::constant(apply(&gm, [c[0] + g.r().get() as f64, c[1]])),
                    stops: stops(g.stops()),
                };
                Some(([1.0; 4], Some(grad)))
            }
            usvg::Paint::Pattern(_) => {
                self.warnings.insert("パターン塗りは無視しました".into());
                None
            }
        }
    }

    fn path(&mut self, p: &usvg::Path, opacity: f64) {
        if !p.is_visible() {
            return;
        }
        let m = svg_affine(p.abs_transform());
        let mut b = Builder::default();
        let pt = |q: usvg::tiny_skia_path::Point| apply(&m, [q.x as f64, q.y as f64]);
        for seg in p.data().segments() {
            use usvg::tiny_skia_path::PathSegment as S;
            match seg {
                S::MoveTo(a) => b.move_to(pt(a)),
                S::LineTo(a) => b.line_to(pt(a)),
                S::QuadTo(c, a) => b.quad_to(pt(c), pt(a)),
                S::CubicTo(c1, c2, a) => b.cubic_to(pt(c1), pt(c2), pt(a)),
                S::Close => b.close(),
            }
        }
        let fill = p.fill().and_then(|f| {
            let (color, gradient) = self.paint(f.paint(), &m)?;
            let rule = if f.rule() == usvg::FillRule::EvenOdd { FillRule::EvenOdd } else { FillRule::NonZero };
            Some(FillStyle { color, opacity: opacity * f.opacity().get() as f64, rule, gradient })
        });
        let stroke = p.stroke().and_then(|s| {
            let (color, gradient) = self.paint(s.paint(), &m)?;
            let k = scale_of(&m);
            Some(StrokeStyle {
                color,
                opacity: opacity * s.opacity().get() as f64,
                width: s.width().get() as f64 * k,
                cap: match s.linecap() {
                    usvg::LineCap::Butt => LineCap::Butt,
                    usvg::LineCap::Round => LineCap::Round,
                    usvg::LineCap::Square => LineCap::Square,
                },
                join: match s.linejoin() {
                    usvg::LineJoin::Round => LineJoin::Round,
                    usvg::LineJoin::Bevel => LineJoin::Bevel,
                    _ => LineJoin::Miter,
                },
                miter_limit: s.miterlimit().get() as f64,
                dashes: s.dasharray().map(|d| d.iter().map(|v| *v as f64 * k).collect()).unwrap_or_default(),
                dash_offset: s.dashoffset() as f64 * k,
                gradient,
            })
        });
        let stroke_first = p.paint_order() == usvg::PaintOrder::StrokeAndFill;
        let name = if p.id().is_empty() { format!("パス {}", self.count + 1) } else { p.id().to_string() };
        if let Some(item) = object(name, b.take(), fill, stroke, stroke_first) {
            self.items.push(item);
            self.count += 1;
        }
    }
}

// ---------- PDF（と PDF 互換 AI） ----------

use lopdf::{Dictionary, Document, Object};

#[derive(Clone)]
struct GState {
    ctm: Affine,
    fill: [f64; 4],
    stroke: [f64; 4],
    fill_alpha: f64,
    stroke_alpha: f64,
    width: f64,
    cap: LineCap,
    join: LineJoin,
    miter: f64,
    dashes: Vec<f64>,
    dash_offset: f64,
    /// 塗り・線がパターン／シェーディングで、単色として扱えない。
    fill_pattern: bool,
    stroke_pattern: bool,
}

struct PdfCx<'a> {
    doc: &'a Document,
    items: Vec<ShapeItem>,
    warnings: BTreeSet<String>,
    count: usize,
}

fn num(o: &Object) -> Option<f64> {
    match o {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) => Some(*r as f64),
        _ => None,
    }
}

fn color_from(operands: &[Object]) -> Option<[f64; 4]> {
    let v: Vec<f64> = operands.iter().filter_map(num).collect();
    match v.len() {
        1 => Some([v[0], v[0], v[0], 1.0]),
        3 => Some([v[0], v[1], v[2], 1.0]),
        // CMYK → RGB（プロファイルなしの単純変換）
        4 => Some([(1.0 - v[0]) * (1.0 - v[3]), (1.0 - v[1]) * (1.0 - v[3]), (1.0 - v[2]) * (1.0 - v[3]), 1.0]),
        _ => None,
    }
}

impl<'a> PdfCx<'a> {
    fn resolve<'o>(&self, o: &'o Object) -> &'o Object
    where
        'a: 'o,
    {
        let doc: &'a Document = self.doc;
        match o {
            Object::Reference(id) => doc.get_object(*id).unwrap_or(o),
            _ => o,
        }
    }

    fn sub_dict<'o>(&self, d: &'o Dictionary, key: &[u8]) -> Option<&'o Dictionary>
    where
        'a: 'o,
    {
        self.resolve(d.get(key).ok()?).as_dict().ok()
    }

    fn run(&mut self, content: &[u8], resources: Option<&Dictionary>, base: Affine, depth: usize) -> Result<(), String> {
        let ops = lopdf::content::Content::decode(content).map_err(|e| format!("PDF の描画命令を解析できません: {e}"))?;
        let mut gs = GState {
            ctm: base,
            fill: [0.0, 0.0, 0.0, 1.0],
            stroke: [0.0, 0.0, 0.0, 1.0],
            fill_alpha: 1.0,
            stroke_alpha: 1.0,
            width: 1.0,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            miter: 10.0,
            dashes: Vec::new(),
            dash_offset: 0.0,
            fill_pattern: false,
            stroke_pattern: false,
        };
        let mut stack: Vec<GState> = Vec::new();
        let mut b = Builder::default();
        for op in &ops.operations {
            let a: Vec<f64> = op.operands.iter().filter_map(num).collect();
            let pt = |gs: &GState, i: usize| apply(&gs.ctm, [a[i], a[i + 1]]);
            match (op.operator.as_str(), a.len()) {
                ("q", _) => stack.push(gs.clone()),
                ("Q", _) => {
                    if let Some(s) = stack.pop() {
                        gs = s;
                    }
                }
                ("cm", 6) => gs.ctm = gs.ctm.then_after(&Affine { a: a[0], b: a[1], c: a[2], d: a[3], tx: a[4], ty: a[5] }),
                ("w", 1) => gs.width = a[0],
                ("J", 1) => gs.cap = [LineCap::Butt, LineCap::Round, LineCap::Square][(a[0] as usize).min(2)],
                ("j", 1) => gs.join = [LineJoin::Miter, LineJoin::Round, LineJoin::Bevel][(a[0] as usize).min(2)],
                ("M", 1) => gs.miter = a[0],
                ("d", _) => {
                    if let Some(Object::Array(arr)) = op.operands.first() {
                        gs.dashes = arr.iter().filter_map(num).collect();
                        gs.dash_offset = op.operands.get(1).and_then(num).unwrap_or(0.0);
                    }
                }
                ("g" | "rg" | "k" | "sc" | "scn", _) => match color_from(&op.operands) {
                    Some(c) if a.len() == op.operands.len() => (gs.fill, gs.fill_pattern) = (c, false),
                    _ => gs.fill_pattern = true,
                },
                ("G" | "RG" | "K" | "SC" | "SCN", _) => match color_from(&op.operands) {
                    Some(c) if a.len() == op.operands.len() => (gs.stroke, gs.stroke_pattern) = (c, false),
                    _ => gs.stroke_pattern = true,
                },
                ("gs", _) => {
                    let name = op.operands.first().and_then(|o| o.as_name().ok());
                    let ext = resources.and_then(|r| self.sub_dict(r, b"ExtGState"));
                    if let Some(d) = name.zip(ext).and_then(|(n, e)| self.sub_dict(e, n)) {
                        if let Some(v) = d.get(b"ca").ok().and_then(num) {
                            gs.fill_alpha = v;
                        }
                        if let Some(v) = d.get(b"CA").ok().and_then(num) {
                            gs.stroke_alpha = v;
                        }
                        if let Some(v) = d.get(b"LW").ok().and_then(num) {
                            gs.width = v;
                        }
                        if d.get(b"BM").ok().and_then(|o| o.as_name().ok()).is_some_and(|n| n != b"Normal") {
                            self.warnings.insert("図形ごとの描画モードは無視しました（通常で合成）".into());
                        }
                        if d.get(b"SMask").ok().is_some_and(|o| o.as_name().ok() != Some(b"None".as_slice())) {
                            self.warnings.insert("ソフトマスク（不透明マスク）は無視しました".into());
                        }
                    }
                }
                ("m", 2) => b.move_to(pt(&gs, 0)),
                ("l", 2) => b.line_to(pt(&gs, 0)),
                ("c", 6) => b.cubic_to(pt(&gs, 0), pt(&gs, 2), pt(&gs, 4)),
                ("v", 4) => {
                    // 最初の制御点が現在位置と同じ
                    if let Some(cur) = b.last() {
                        b.cubic_to(cur, pt(&gs, 0), pt(&gs, 2));
                    }
                }
                ("y", 4) => {
                    let end = pt(&gs, 2);
                    b.cubic_to(pt(&gs, 0), end, end);
                }
                ("h", _) => b.close(),
                ("re", 4) => {
                    let (x, y, w, h) = (a[0], a[1], a[2], a[3]);
                    b.move_to(apply(&gs.ctm, [x, y]));
                    b.line_to(apply(&gs.ctm, [x + w, y]));
                    b.line_to(apply(&gs.ctm, [x + w, y + h]));
                    b.line_to(apply(&gs.ctm, [x, y + h]));
                    b.close();
                }
                ("W" | "W*", _) => {
                    self.warnings.insert("クリッピングパスは無視しました".into());
                }
                ("n", _) => {
                    b.take();
                }
                (paint @ ("f" | "F" | "f*" | "B" | "B*" | "b" | "b*" | "S" | "s"), _) => {
                    if matches!(paint, "b" | "b*" | "s") {
                        b.close();
                    }
                    let fills = matches!(paint, "f" | "F" | "f*" | "B" | "B*" | "b" | "b*");
                    let strokes = matches!(paint, "B" | "B*" | "b" | "b*" | "S" | "s");
                    let rule = if paint.ends_with('*') { FillRule::EvenOdd } else { FillRule::NonZero };
                    if (fills && gs.fill_pattern) || (strokes && gs.stroke_pattern) {
                        self.warnings.insert("パターン・グラデーション（シェーディング）の塗りは無視しました".into());
                    }
                    let fill =
                        (fills && !gs.fill_pattern).then(|| FillStyle { color: gs.fill, opacity: gs.fill_alpha, rule, gradient: None });
                    let k = scale_of(&gs.ctm);
                    let stroke = (strokes && !gs.stroke_pattern).then(|| StrokeStyle {
                        color: gs.stroke,
                        opacity: gs.stroke_alpha,
                        // 幅 0 は「デバイスで最も細い線」
                        width: (gs.width * k).max(0.25),
                        cap: gs.cap,
                        join: gs.join,
                        miter_limit: gs.miter,
                        dashes: gs.dashes.iter().map(|d| d * k).collect(),
                        dash_offset: gs.dash_offset * k,
                        gradient: None,
                    });
                    if let Some(item) = object(format!("パス {}", self.count + 1), b.take(), fill, stroke, false) {
                        self.items.push(item);
                        self.count += 1;
                    }
                }
                ("sh", _) => {
                    self.warnings.insert("パターン・グラデーション（シェーディング）の塗りは無視しました".into());
                }
                ("BT", _) => {
                    self.warnings.insert("テキストは読み込んでいません（書き出し前にアウトライン化してください）".into());
                }
                ("BI", _) => {
                    self.warnings.insert("埋め込み画像は読み込んでいません".into());
                }
                ("Do", _) => {
                    let name = op.operands.first().and_then(|o| o.as_name().ok());
                    let xobjects = resources.and_then(|r| self.sub_dict(r, b"XObject"));
                    let Some(Object::Stream(stream)) = name.zip(xobjects).and_then(|(n, x)| x.get(n).ok()).map(|o| self.resolve(o)) else {
                        continue;
                    };
                    let subtype = stream.dict.get(b"Subtype").ok().and_then(|o| o.as_name().ok());
                    if subtype != Some(b"Form".as_slice()) {
                        self.warnings.insert("埋め込み画像は読み込んでいません".into());
                        continue;
                    }
                    if depth > 16 {
                        continue;
                    }
                    let m: Vec<f64> = match stream.dict.get(b"Matrix").ok().map(|o| self.resolve(o)) {
                        Some(Object::Array(arr)) => arr.iter().filter_map(num).collect(),
                        _ => Vec::new(),
                    };
                    let form_m =
                        if m.len() == 6 { Affine { a: m[0], b: m[1], c: m[2], d: m[3], tx: m[4], ty: m[5] } } else { Affine::IDENTITY };
                    let stream = stream.clone();
                    let data = stream.decompressed_content().unwrap_or_else(|_| stream.content.clone());
                    let own = stream.dict.get(b"Resources").ok().map(|o| self.resolve(o).clone());
                    let res = own.as_ref().and_then(|o| o.as_dict().ok()).or(resources).cloned();
                    self.run(&data, res.as_ref(), gs.ctm.then_after(&form_m), depth + 1)?;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

fn import_pdf(data: &[u8]) -> Result<VectorImport, String> {
    let doc = Document::load_mem(data).map_err(|e| format!("PDF を解析できません: {e}"))?;
    let pages = doc.get_pages();
    let (_, &page_id) = pages.iter().next().ok_or("ページがありません")?;
    let mut cx = PdfCx { doc: &doc, items: Vec::new(), warnings: BTreeSet::new(), count: 0 };
    if pages.len() > 1 {
        cx.warnings.insert(format!("{} ページ中の 1 ページ目だけを読み込みました", pages.len()));
    }
    // MediaBox は親ノードから継承されることがある
    let mut media_box = None;
    let mut node = doc.get_dictionary(page_id).ok();
    for _ in 0..32 {
        let Some(d) = node else { break };
        if let Some(Object::Array(arr)) = d.get(b"MediaBox").ok().map(|o| cx.resolve(o)) {
            let v: Vec<f64> = arr.iter().filter_map(num).collect();
            if v.len() == 4 {
                media_box = Some([v[0], v[1], v[2], v[3]]);
                break;
            }
        }
        node = d.get(b"Parent").ok().and_then(|p| p.as_reference().ok()).and_then(|id| doc.get_dictionary(id).ok());
    }
    let [x0, y0, x1, y1] = media_box.unwrap_or([0.0, 0.0, 612.0, 792.0]);
    let (width, height) = ((x1 - x0).abs(), (y1 - y0).abs());
    // PDF は左下原点・上向き。左上原点・下向きへ
    let base = Affine { a: 1.0, b: 0.0, c: 0.0, d: -1.0, tx: -x0.min(x1), ty: y0.max(y1) };
    let content = doc.get_page_content(page_id).map_err(|e| format!("ページの内容を読めません: {e}"))?;
    let resources = doc.get_page_resources(page_id);
    let res: Option<Dictionary> = match resources {
        Ok((Some(d), _)) => Some(d.clone()),
        Ok((None, ids)) => ids.first().and_then(|id| doc.get_dictionary(*id).ok()).cloned(),
        Err(_) => None,
    };
    cx.run(&content, res.as_ref(), base, 0)?;
    cx.items.reverse();
    Ok(VectorImport {
        name: String::new(),
        width,
        height,
        items: cx.items,
        path_count: cx.count,
        warnings: cx.warnings.into_iter().collect(),
    })
}
