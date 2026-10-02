//! テキストレイヤーの描画。レイアウト結果の各字形に、アニメーターの変化を合成して描く。

use crate::blur::{gaussian, Edge};
use crate::color::ColorXf;
use crate::frame::FrameBuf;
use crate::path::{fill_coverage, flatten, stroke_outline, Coverage, Polyline};
use crate::shape::{paint_coverage, transform_path, Paint};
use znd_core::shape::{BezPath, FillRule, LineCap, LineJoin};
use znd_core::text::{char_infos, selector_amounts, TextAnimator, TextDocument, TextPath};
use znd_core::{Affine, Time};
use znd_text::{layout, Layout};

pub struct TextCtx<'a> {
    pub cx: &'a ColorXf,
    /// エクスプレッションセレクターの評価関数（`znd_core::text::selector_amounts` 参照）。
    pub selector_expr: &'a dyn Fn(&str, usize, usize, f64) -> f64,
    /// 文字ごとの 3D で使う遠近の基準距離（px）。
    pub zoom: f64,
}

fn lerp4(a: [f64; 4], b: [f64; 4], t: f64) -> [f64; 4] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, a[3] + (b[3] - a[3]) * t]
}

/// アニメーター適用後の文字列・文字ごとの適用量・レイアウトを求める。
pub fn animated_layout(
    doc: &TextDocument,
    animators: &[TextAnimator],
    t: Time,
    expr: &dyn Fn(&str, usize, usize, f64) -> f64,
) -> (Layout, Vec<Vec<f64>>) {
    let infos = char_infos(&doc.text);
    let amounts: Vec<Vec<f64>> = animators.iter().map(|a| selector_amounts(&a.selectors, &infos, t, expr)).collect();

    // 文字オフセット: コードポイントをずらした文字列で組む
    let needs_offset = animators.iter().any(|a| a.props.character_offset.is_some());
    let text = if needs_offset {
        let mut i = 0;
        let s: String = doc
            .text
            .chars()
            .map(|ch| {
                if ch == '\n' {
                    return ch;
                }
                let off: f64 =
                    animators.iter().zip(&amounts).filter_map(|(a, am)| a.props.character_offset.as_ref().map(|p| p.eval(t) * am[i])).sum();
                i += 1;
                if ch.is_whitespace() {
                    ch
                } else {
                    char::from_u32((ch as i64 + off.round() as i64).max(0x21) as u32).unwrap_or(ch)
                }
            })
            .collect();
        Some(s)
    } else {
        None
    };

    let tracking: Vec<f64> = (0..infos.len())
        .map(|i| animators.iter().zip(&amounts).filter_map(|(a, am)| a.props.tracking.as_ref().map(|p| p.eval(t) * am[i])).sum())
        .collect();
    let lay = layout(doc, text.as_deref(), &|i| tracking.get(i).copied().unwrap_or(0.0), t);
    (lay, amounts)
}

/// 弧長 `s` でのパス上の点と接線角（度）。範囲外は、開いたパスなら端の接線方向に延長、閉じたパスなら周回。
fn point_on(line: &Polyline, s: f64) -> ([f64; 2], f64) {
    let total = line.length();
    if total <= 0.0 || line.pts.len() < 2 {
        return (line.pts.first().copied().unwrap_or([0.0, 0.0]), 0.0);
    }
    let s = if line.closed { s.rem_euclid(total) } else { s };
    let edges: Vec<([f64; 2], [f64; 2])> = line.edges().collect();
    let mut acc = 0.0;
    for (i, (a, b)) in edges.iter().enumerate() {
        let l = (b[0] - a[0]).hypot(b[1] - a[1]);
        let last = i + 1 == edges.len();
        if (s <= acc + l || last) && (s >= acc || i == 0) && l > 0.0 {
            let u = (s - acc) / l;
            let angle = (b[1] - a[1]).atan2(b[0] - a[0]).to_degrees();
            return ([a[0] + (b[0] - a[0]) * u, a[1] + (b[1] - a[1]) * u], angle);
        }
        acc += l;
    }
    (line.pts[line.pts.len() - 1], 0.0)
}

fn blurred(cov: Coverage, sigma: f32, w: usize, h: usize) -> Coverage {
    if sigma <= 0.01 || cov.w == 0 {
        return cov;
    }
    // ぼかしの裾が入るよう領域を広げる
    let pad = (sigma * 3.0).ceil() as usize + 1;
    let x0 = cov.x0.saturating_sub(pad);
    let y0 = cov.y0.saturating_sub(pad);
    let x1 = (cov.x0 + cov.w + pad).min(w);
    let y1 = (cov.y0 + cov.h + pad).min(h);
    let (nw, nh) = (x1 - x0, y1 - y0);
    let mut data = vec![0.0f32; nw * nh];
    for row in 0..cov.h {
        let dst = (cov.y0 - y0 + row) * nw + (cov.x0 - x0);
        data[dst..dst + cov.w].copy_from_slice(&cov.data[row * cov.w..(row + 1) * cov.w]);
    }
    gaussian(&mut data, nw, nh, 1, sigma, sigma, Edge::Zero);
    Coverage { x0, y0, w: nw, h: nh, data }
}

/// テキストをキャンバスに描く。`m` はレイヤー座標 → 出力画素。`path` はパス上テキストのパス（レイヤー座標）。
#[allow(clippy::too_many_arguments)]
pub fn render_text(
    doc: &TextDocument,
    animators: &[TextAnimator],
    path: Option<(&TextPath, &BezPath)>,
    per_char_3d: bool,
    t: Time,
    m: &Affine,
    canvas: &mut FrameBuf,
    ctx: &TextCtx,
) {
    let (lay, amounts) = animated_layout(doc, animators, t, ctx.selector_expr);
    let (w, h) = (canvas.width, canvas.height);
    let m_scale = (m.a * m.d - m.b * m.c).abs().sqrt().max(1e-6);
    let path_line = path.map(|(opts, p)| {
        let mut line = flatten(p, 0.05 / m_scale);
        if opts.reverse {
            line.pts.reverse();
        }
        (opts, line)
    });

    for g in &lay.glyphs {
        let i = g.char_index;
        let mut pos = [0.0f64; 2];
        let mut anchor = [0.0f64; 2];
        let mut scale = [1.0f64; 2];
        let mut rot = 0.0f64;
        let mut opacity = 1.0f64;
        let mut fill = doc.fill;
        let mut stroke_color = doc.stroke.as_ref().map(|s| s.color);
        let mut stroke_width = doc.stroke.as_ref().map(|s| s.width).unwrap_or(0.0);
        let mut blur = 0.0f64;
        let (mut z, mut rx, mut ry) = (0.0f64, 0.0f64, 0.0f64);

        for (anim, am) in animators.iter().zip(&amounts) {
            let a = am.get(i).copied().unwrap_or(0.0);
            if a == 0.0 {
                continue;
            }
            let p = &anim.props;
            if let Some(v) = &p.position {
                let v = v.eval(t);
                pos = [pos[0] + v[0] * a, pos[1] + v[1] * a];
            }
            if let Some(v) = &p.anchor {
                let v = v.eval(t);
                anchor = [anchor[0] + v[0] * a, anchor[1] + v[1] * a];
            }
            if let Some(v) = &p.scale {
                let v = v.eval(t);
                scale = [scale[0] * (1.0 + a * (v[0] / 100.0 - 1.0)), scale[1] * (1.0 + a * (v[1] / 100.0 - 1.0))];
            }
            if let Some(v) = &p.rotation {
                rot += v.eval(t) * a;
            }
            if let Some(v) = &p.opacity {
                opacity *= (1.0 + a * (v.eval(t) / 100.0 - 1.0)).clamp(0.0, 1.0);
            }
            if let Some(v) = &p.fill_color {
                let c = v.eval(t);
                fill = Some(lerp4(fill.unwrap_or([c[0], c[1], c[2], 0.0]), c, a.abs().min(1.0)));
            }
            if let Some(v) = &p.stroke_color {
                let c = v.eval(t);
                stroke_color = Some(lerp4(stroke_color.unwrap_or([c[0], c[1], c[2], 0.0]), c, a.abs().min(1.0)));
            }
            if let Some(v) = &p.stroke_width {
                stroke_width += v.eval(t) * a;
            }
            if let Some(v) = &p.blur {
                blur += v.eval(t) * a;
            }
            if per_char_3d {
                z += p.position_z.as_ref().map(|v| v.eval(t) * a).unwrap_or(0.0);
                rx += p.rotation_x.as_ref().map(|v| v.eval(t) * a).unwrap_or(0.0);
                ry += p.rotation_y.as_ref().map(|v| v.eval(t) * a).unwrap_or(0.0);
            }
        }
        if opacity <= 0.0 {
            continue;
        }
        if per_char_3d {
            // 文字ごとの 3D（正射影近似）: X/Y 回転は幅・高さの短縮、Z 位置は遠近による拡大縮小
            let persp = if ctx.zoom + z > 1.0 { ctx.zoom / (ctx.zoom + z) } else { 0.0 };
            scale = [scale[0] * ry.to_radians().cos() * persp, scale[1] * rx.to_radians().cos() * persp];
        }

        // em 単位の字形 → レイヤー座標
        let mut to_layer = Affine::translate(g.origin[0], g.origin[1]).then_after(&Affine::scale(g.scale, g.scale));
        let mut center = g.center;
        if let Some((opts, line)) = &path_line {
            // 文字の中心の x を弧長とみなしてパス上に置く
            let s = opts.first_margin.eval(t) + g.center[0];
            let (p, angle) = point_on(line, s);
            let place = Affine::translate(p[0], p[1])
                .then_after(&Affine::rotate(if opts.perpendicular { angle } else { 0.0 }))
                .then_after(&Affine::translate(-g.center[0], 0.0));
            to_layer = place.then_after(&to_layer);
            let c = place.apply(center[0], center[1]);
            center = [c.0, c.1];
        }
        let animate = Affine::translate(center[0] + pos[0], center[1] + pos[1])
            .then_after(&Affine::rotate(rot))
            .then_after(&Affine::scale(scale[0], scale[1]))
            .then_after(&Affine::translate(-center[0] - anchor[0], -center[1] - anchor[1]));
        let glyph_to_layer = animate.then_after(&to_layer);

        let em_scale = (glyph_to_layer.a * glyph_to_layer.d - glyph_to_layer.b * glyph_to_layer.c).abs().sqrt().max(1e-9);
        let tol = 0.03 / (em_scale * m_scale);
        // レイヤー座標の折れ線（線の太さはレイヤー座標で付ける）
        let layer_lines: Vec<Polyline> = g.paths.iter().map(|p| flatten(&transform_path(p, &glyph_to_layer), tol * em_scale)).collect();
        let sigma = (blur * m_scale / 2.0) as f32;

        if let Some(c) = fill.filter(|c| c[3] > 0.0) {
            let polys: Vec<Polyline> = layer_lines.iter().map(|l| l.transformed(m)).collect();
            let cov = blurred(fill_coverage(&polys, FillRule::NonZero, w, h), sigma, w, h);
            paint_coverage(canvas, &cov, &Paint::Solid(ctx.cx.premul(c)), opacity as f32);
        }
        if let Some(c) = stroke_color.filter(|c| c[3] > 0.0 && stroke_width > 0.0) {
            let polys: Vec<Polyline> = layer_lines
                .iter()
                .flat_map(|l| stroke_outline(l, stroke_width, LineCap::Round, LineJoin::Round, 4.0, 0.03 / m_scale))
                .map(|p| p.transformed(m))
                .collect();
            let cov = blurred(fill_coverage(&polys, FillRule::NonZero, w, h), sigma, w, h);
            paint_coverage(canvas, &cov, &Paint::Solid(ctx.cx.premul(c)), opacity as f32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use znd_core::shape::PathVertex;
    use znd_core::text::{AnimatorProps, Justify, RangeShape, SelectorMode, SelectorUnit, TextSelector};
    use znd_core::Property;

    fn ctx() -> TextCtx<'static> {
        TextCtx { cx: &ColorXf::SRGB, selector_expr: &|_, _, _, v| v, zoom: 1000.0 }
    }

    fn draw(doc: &TextDocument, animators: &[TextAnimator], t: f64) -> FrameBuf {
        let mut c = FrameBuf::new(400, 200);
        render_text(doc, animators, None, false, Time::from_seconds(t), &Affine::translate(20.0, 120.0), &mut c, &ctx());
        c
    }

    fn ink(c: &FrameBuf) -> f64 {
        c.data.chunks_exact(4).map(|p| p[3] as f64).sum()
    }

    /// インクの重心。
    fn centroid(c: &FrameBuf) -> (f64, f64) {
        let (mut sx, mut sy, mut s) = (0.0, 0.0, 0.0);
        for y in 0..c.height {
            for x in 0..c.width {
                let a = c.pixel(x, y)[3] as f64;
                sx += a * x as f64;
                sy += a * y as f64;
                s += a;
            }
        }
        (sx / s.max(1e-9), sy / s.max(1e-9))
    }

    fn have_fonts() -> bool {
        ink(&draw(&TextDocument::new("A", 80.0), &[], 0.0)) > 0.0
    }

    fn range(start: f64, end: f64) -> TextSelector {
        TextSelector::Range {
            start: Property::constant(start),
            end: Property::constant(end),
            offset: Property::constant(0.0),
            amount: Property::constant(100.0),
            unit: SelectorUnit::Characters,
            shape: RangeShape::Square,
            mode: SelectorMode::Add,
            randomize: false,
            seed: 0,
        }
    }

    #[test]
    fn plain_text_draws_ink_in_fill_color() {
        if !have_fonts() {
            return;
        }
        let mut doc = TextDocument::new("音ハメ", 80.0);
        doc.fill = Some([1.0, 0.0, 0.0, 1.0]);
        let c = draw(&doc, &[], 0.0);
        assert!(ink(&c) > 2000.0);
        // 完全に覆われた画素は純赤
        let solid = c.data.chunks_exact(4).find(|p| p[3] == 1.0).unwrap();
        assert_eq!(solid, &[1.0, 0.0, 0.0, 1.0]);
        // 線を付けるとインクが増える
        doc.stroke = Some(znd_core::text::TextStroke { color: [1.0, 1.0, 1.0, 1.0], width: 6.0 });
        assert!(ink(&draw(&doc, &[], 0.0)) > ink(&c) * 1.2);
    }

    #[test]
    fn animator_moves_only_selected_characters() {
        if !have_fonts() {
            return;
        }
        let doc = TextDocument::new("AAAA", 80.0);
        let base = draw(&doc, &[], 0.0);
        let anim = |sel: TextSelector| TextAnimator {
            name: "a".into(),
            selectors: vec![sel],
            props: AnimatorProps { position: Some(Property::constant([0.0, -40.0])), ..Default::default() },
        };
        // 全選択: 全体が 40px 上へ
        let all = draw(&doc, &[anim(range(0.0, 100.0))], 0.0);
        assert!((centroid(&all).1 - (centroid(&base).1 - 40.0)).abs() < 0.5);
        assert!((ink(&all) - ink(&base)).abs() < 1.0);
        // 前半 2 文字だけ: 重心は 20px 上がる
        let half = draw(&doc, &[anim(range(0.0, 50.0))], 0.0);
        assert!((centroid(&half).1 - (centroid(&base).1 - 20.0)).abs() < 0.5);
    }

    #[test]
    fn opacity_scale_and_tracking_animators() {
        if !have_fonts() {
            return;
        }
        let doc = TextDocument::new("AAAA", 80.0);
        let base = draw(&doc, &[], 0.0);
        let with = |props: AnimatorProps| draw(&doc, &[TextAnimator { name: String::new(), selectors: vec![], props }], 0.0);
        let faded = with(AnimatorProps { opacity: Some(Property::constant(25.0)), ..Default::default() });
        assert!((ink(&faded) / ink(&base) - 0.25).abs() < 0.01);
        let small = with(AnimatorProps { scale: Some(Property::constant([50.0, 50.0])), ..Default::default() });
        assert!((ink(&small) / ink(&base) - 0.25).abs() < 0.02);
        let wide = with(AnimatorProps { tracking: Some(Property::constant(500.0)), ..Default::default() });
        assert!(centroid(&wide).0 > centroid(&base).0 + 40.0);
        let blurry = with(AnimatorProps { blur: Some(Property::constant(10.0)), ..Default::default() });
        let max = |c: &FrameBuf| c.data.chunks_exact(4).map(|p| p[3]).fold(0.0f32, f32::max);
        assert!(max(&blurry) < max(&base) && (ink(&blurry) - ink(&base)).abs() / ink(&base) < 0.05);
    }

    #[test]
    fn character_offset_changes_glyphs() {
        if !have_fonts() {
            return;
        }
        let a = draw(&TextDocument::new("A", 80.0), &[], 0.0);
        let shifted = draw(
            &TextDocument::new("A", 80.0),
            &[TextAnimator {
                name: String::new(),
                selectors: vec![],
                props: AnimatorProps { character_offset: Some(Property::constant(1.0)), ..Default::default() },
            }],
            0.0,
        );
        let b = draw(&TextDocument::new("B", 80.0), &[], 0.0);
        assert_eq!(shifted, b);
        assert_ne!(shifted, a);
    }

    #[test]
    fn text_follows_a_path() {
        if !have_fonts() {
            return;
        }
        let mut doc = TextDocument::new("AAAAAA", 40.0);
        doc.justify = Justify::Left;
        // 下向きに伸びる縦の直線。文字は上から下へ並び、90° 回転する
        let path = BezPath { vertices: vec![PathVertex::corner(100.0, 10.0), PathVertex::corner(100.0, 190.0)], closed: false };
        let opts = TextPath { mask: 0, reverse: false, perpendicular: true, first_margin: Property::constant(0.0) };
        let mut c = FrameBuf::new(200, 200);
        render_text(&doc, &[], Some((&opts, &path)), false, Time::ZERO, &Affine::IDENTITY, &mut c, &ctx());
        let (cx, cy) = centroid(&c);
        assert!(cy > 60.0 && cy < 120.0, "cy {cy}");
        assert!((cx - 114.0).abs() < 12.0, "cx {cx}");
        // インクは縦長に分布する
        let ys: Vec<usize> = (0..200).filter(|y| (0..200).any(|x| c.pixel(x, *y)[3] > 0.5)).collect();
        assert!(ys.last().unwrap() - ys.first().unwrap() > 120);
    }
}
