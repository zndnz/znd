//! タイムラインパネル。左はレイヤーの列（表示・ソロ・ロック・ラベル・番号・名前・スイッチ・モード・マット・親）、
//! 右は時間（ワークエリア・ルーラー・マーカー・レイヤーのバー・プロパティごとのキーフレーム）。
//!
//! レイヤーは展開でき、トランスフォームやエフェクトなどのグループの下にプロパティの行が並ぶ。
//! 横方向は拡大・スクロールできる（⌘ / Alt + ホイールで拡大、ホイールで横スクロール）。最大まで拡大すると
//! 波形が 1 サンプルずつ見える。レイヤーのバーは掴んで移動・端を掴んでトリム、キーフレームは掴んで移動でき、
//! いずれもビートグリッド・マーカー・再生ヘッド・他のレイヤーの端に吸着する（Shift で吸着なし）。
//! 左のレイヤーの行は、上下にドラッグで重ね順の並び替え、右クリックで複製・削除などのメニュー。

use crate::icons::{self, Icon};
use crate::keymap::Action;
use crate::panels::{apply_edit, collect_rows, path_label, value_widgets, KeyKind, PropEdit, Row};
use crate::{theme, widgets, App};
use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, FontId, Layout, Rect, Sense, Shape, Stroke, UiBuilder};
use znd_audio::analysis::spectrogram;
use znd_audio::SAMPLE_RATE;
use znd_core::*;
use znd_core::keys;

/// 左ペイン（レイヤーの列）の既定の幅。
pub(crate) const LEFT_DEFAULT: f32 = 520.0;
const ROW: f32 = theme::ROW_H;
const HEAD_H: f32 = 26.0;
const COLS_H: f32 = 18.0;
const FOOT_H: f32 = 15.0;
const AUDIO_ROW: f32 = 40.0;
/// ルーラー上端の、ワークエリアのバーの高さ。
const WORK_H: f32 = 7.0;

/// 最大拡大時の 1 フレームの幅（px）。30fps・48kHz で 1 サンプルが 1px を超える。
const MAX_PX_PER_FRAME: f64 = 2400.0;
/// バーの端・キーフレームを掴める範囲（px）。
const GRAB: f32 = 5.0;

/// タイムラインの横方向の表示範囲。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TlView {
    pub left: f32,
    pub width: f32,
    /// 左端のフレーム（小数）と、表示しているフレーム数。
    pub start: f64,
    pub span: f64,
}

impl TlView {
    pub fn x(&self, frame: f64) -> f32 {
        self.left + ((frame - self.start) / self.span) as f32 * self.width
    }
    pub fn frame(&self, x: f32) -> f64 {
        self.start + ((x - self.left) / self.width) as f64 * self.span
    }
    pub fn px_per_frame(&self) -> f64 {
        self.width as f64 / self.span
    }
}

/// 選択中のキーフレーム 1 つ（レイヤー・プロパティのパス・時刻）。
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct KeySel {
    pub layer: LayerId,
    pub path: String,
    pub at: Time,
}

/// タイムラインでドラッグ中の操作。
pub(crate) enum TlDrag {
    /// レイヤー全体の移動。`grab` は掴んだ位置の、イン点からのずれ。
    Move {
        layer: LayerId,
        grab: Time,
    },
    TrimIn {
        layer: LayerId,
    },
    TrimOut {
        layer: LayerId,
    },
    /// 選択中のキーフレームの移動（`pivot` があれば、そこを中心にした伸縮）。
    /// `grab` は掴んだキーの元の時刻、`last` はいまの時刻。`orig` / `base` はドラッグを始めたときの選択とレイヤー。
    Keys {
        grab: Time,
        last: Time,
        pivot: Option<Time>,
        orig: Vec<KeySel>,
        base: Vec<Layer>,
    },
    /// 何もないところからドラッグして、矩形の中のキーフレームを選ぶ。`base` は始めたときの選択（Shift / ⌘ で足す場合）。
    Marquee {
        start: egui::Pos2,
        base: Vec<KeySel>,
    },
    /// 左の行を掴んで、重ね順を並び替える。
    Reorder {
        layer: LayerId,
    },
}

/// 左ペインの列の位置。
#[derive(Clone, Copy)]
struct Geo {
    x0: f32,
    /// 左ペインの右端。
    split: f32,
    show_parent: bool,
    show_matte: bool,
}

const AV_W: f32 = 16.0;
const SW_W: f32 = 16.0;
const MODE_W: f32 = 66.0;
const MATTE_W: f32 = 66.0;
const PARENT_W: f32 = 78.0;
/// 展開の三角・プロパティのインデントが始まる位置。
const TREE_X: f32 = 88.0;

impl Geo {
    fn parent(&self) -> f32 {
        self.split - 2.0 - if self.show_parent { PARENT_W } else { 0.0 }
    }
    fn matte(&self) -> f32 {
        self.parent() - if self.show_matte { MATTE_W } else { 0.0 }
    }
    fn mode(&self) -> f32 {
        self.matte() - MODE_W
    }
    fn switches(&self) -> f32 {
        self.mode() - 4.0 * SW_W - 4.0
    }
    fn cell(&self, x: f32, w: f32, row: Rect) -> Rect {
        Rect::from_min_max(pos2(x, row.top()), pos2(x + w, row.bottom()))
    }
}

fn kind_icon(kind: &LayerKind) -> Icon {
    match kind {
        LayerKind::Solid { .. } => Icon::Solid,
        LayerKind::Footage { .. } => Icon::Footage,
        LayerKind::Precomp { .. } => Icon::Comp,
        LayerKind::Null => Icon::Null,
        LayerKind::Adjustment => Icon::Adjust,
        LayerKind::Shape { .. } => Icon::Star,
        LayerKind::Text { .. } => Icon::Text,
        LayerKind::Audio { .. } => Icon::Audio,
        LayerKind::Camera { .. } => Icon::Camera,
        LayerKind::Light { .. } => Icon::Light,
        LayerKind::Model { .. } => Icon::Model,
    }
}

/// レイヤーの全プロパティのキー時刻（重複なし・昇順）。
fn key_times(layer: &Layer) -> Vec<Time> {
    let mut times = Vec::new();
    visit::visit_layer(layer, &mut |_, p| times.extend(p.key_times()));
    times.sort();
    times.dedup();
    times
}

/// `時:分:秒:フレーム` 表記。
pub(crate) fn timecode(fr: FrameRate, frame: i64) -> String {
    let fps = (fr.fps().round() as i64).max(1);
    let (s, f) = (frame.div_euclid(fps), frame.rem_euclid(fps));
    format!("{}:{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60, f)
}

/// ルーラーの目盛りの間隔（大・中・小、フレーム単位）。中目盛りが無ければ 0。
fn tick_steps(fps: f64, px_per_frame: f64) -> (i64, i64, i64) {
    let f = (fps.round() as i64).max(1);
    let mut cands = vec![1, 2, 5, 10, f / 2, f, 2 * f, 5 * f, 10 * f, 15 * f, 30 * f, 60 * f, 120 * f, 300 * f, 600 * f];
    cands.retain(|c| *c > 0);
    cands.sort();
    cands.dedup();
    let major = cands.iter().copied().find(|c| *c as f64 * px_per_frame >= 64.0).unwrap_or(600 * f);
    let minor = [10, 6, 5, 4, 3, 2]
        .into_iter()
        .find(|d| major % d == 0 && (major / d) as f64 * px_per_frame >= 4.0)
        .map(|d| major / d)
        .unwrap_or(major);
    let mid = if major % 2 == 0 && major / 2 != minor && (major / 2) % minor == 0 { major / 2 } else { 0 };
    (major, mid, minor)
}

/// キーフレームの印。補間の種類で形を変える（リニア = ひし形、停止 = 四角、イージーイーズ = 砂時計、
/// イーズイン／アウト = 片側が平らな五角形、自動ベジェ = 円）。選択中は塗りつぶす。
fn key_shape(p: &egui::Painter, c: egui::Pos2, kind: KeyKind, selected: bool) {
    let r = 4.5;
    let (fill, stroke) = if selected {
        (theme::KEY_SELECTED, Stroke::new(1.0_f32, Color32::WHITE))
    } else {
        (theme::BG0, Stroke::new(1.2_f32, Color32::from_gray(0xc4)))
    };
    let poly = |pts: Vec<egui::Pos2>| {
        p.add(Shape::convex_polygon(pts, fill, stroke));
    };
    match kind {
        KeyKind::Linear => poly(vec![pos2(c.x, c.y - r), pos2(c.x + r, c.y), pos2(c.x, c.y + r), pos2(c.x - r, c.y)]),
        KeyKind::Hold => {
            poly(vec![pos2(c.x - 3.5, c.y - 3.5), pos2(c.x + 3.5, c.y - 3.5), pos2(c.x + 3.5, c.y + 3.5), pos2(c.x - 3.5, c.y + 3.5)])
        }
        KeyKind::EasyEase => {
            poly(vec![pos2(c.x - 4.0, c.y - r), pos2(c.x + 4.0, c.y - r), pos2(c.x, c.y + 0.5)]);
            poly(vec![pos2(c.x - 4.0, c.y + r), pos2(c.x, c.y - 0.5), pos2(c.x + 4.0, c.y + r)]);
        }
        KeyKind::EaseIn => {
            poly(vec![pos2(c.x - 3.5, c.y - r), pos2(c.x, c.y - r), pos2(c.x + r, c.y), pos2(c.x, c.y + r), pos2(c.x - 3.5, c.y + r)])
        }
        KeyKind::EaseOut => {
            poly(vec![pos2(c.x + 3.5, c.y - r), pos2(c.x + 3.5, c.y + r), pos2(c.x, c.y + r), pos2(c.x - r, c.y), pos2(c.x, c.y - r)])
        }
        KeyKind::AutoBezier => {
            p.circle(c, 4.0, fill, stroke);
        }
    }
}

/// 展開したレイヤーの下に並べる行。
struct Node {
    depth: usize,
    kind: NodeKind,
}

enum NodeKind {
    Group {
        key: String,
        label: String,
        default_open: bool,
        /// 配下のプロパティのキー時刻（畳んだときの目印）。
        times: Vec<Time>,
        /// エフェクトのグループなら、その番号。
        fx: Option<usize>,
    },
    /// `rows` の添字。
    Prop(usize),
}

fn layer_tree(layer: &Layer, rows: &[Row], three: bool, audio: bool, only_animated: bool) -> Vec<Node> {
    let keep = |r: &Row| !only_animated || r.animated || r.expr.is_some();
    let pick = |f: &dyn Fn(&str) -> bool| -> Vec<usize> {
        rows.iter().enumerate().filter(|(_, r)| f(&r.path) && keep(r)).map(|(i, _)| i).collect()
    };
    let times = |idx: &[usize]| -> Vec<Time> {
        let mut t: Vec<Time> = idx.iter().flat_map(|i| rows[*i].keys.iter().map(|k| k.0)).collect();
        t.sort();
        t.dedup();
        t
    };
    let lid = &layer.id.0;
    let group = |out: &mut Vec<Node>, name: &str, label: &str, depth: usize, default_open: bool, fx: Option<usize>, idx: &[usize]| {
        out.push(Node {
            depth,
            kind: NodeKind::Group { key: format!("{lid}/{name}"), label: label.to_string(), default_open, times: times(idx), fx },
        });
    };
    let leaf = |out: &mut Vec<Node>,
                name: &str,
                label: &str,
                depth: usize,
                default_open: bool,
                fx: Option<usize>,
                idx: Vec<usize>,
                force: bool| {
        if idx.is_empty() && !force {
            return;
        }
        group(out, name, label, depth, default_open, fx, &idx);
        out.extend(idx.into_iter().map(|i| Node { depth: depth + 1, kind: NodeKind::Prop(i) }));
    };

    let mut out = Vec::new();
    for i in pick(&|p| p == "time_remap") {
        out.push(Node { depth: 1, kind: NodeKind::Prop(i) });
    }
    match &layer.kind {
        LayerKind::Text { animators, .. } => {
            let all = pick(&|p| p.starts_with("text."));
            if !all.is_empty() {
                group(&mut out, "text", "テキスト", 1, false, None, &all);
                for i in pick(&|p| p.starts_with("text.path.")) {
                    out.push(Node { depth: 2, kind: NodeKind::Prop(i) });
                }
                for (i, a) in animators.iter().enumerate() {
                    let prefix = format!("text.{i}.");
                    leaf(&mut out, &format!("text.{i}"), &a.name, 2, true, None, pick(&|p| p.starts_with(&prefix)), false);
                }
            }
        }
        LayerKind::Shape { .. } => leaf(&mut out, "shape", "コンテンツ", 1, false, None, pick(&|p| p.starts_with("shape.")), false),
        _ => {}
    }
    if !layer.masks.is_empty() {
        let all = pick(&|p| p.starts_with("mask."));
        if !all.is_empty() {
            group(&mut out, "mask", "マスク", 1, false, None, &all);
            for i in 0..layer.masks.len() {
                let prefix = format!("mask.{i}.");
                leaf(&mut out, &format!("mask.{i}"), &format!("マスク {}", i + 1), 2, true, None, pick(&|p| p.starts_with(&prefix)), false);
            }
        }
    }
    if !layer.effects.is_empty() {
        let all = pick(&|p| p.starts_with("fx."));
        if !all.is_empty() || !only_animated {
            group(&mut out, "fx", "エフェクト", 1, false, None, &all);
            for (i, fx) in layer.effects.iter().enumerate() {
                let prefix = format!("fx.{i}.");
                leaf(&mut out, &format!("fx.{i}"), &fx.name, 2, true, Some(i), pick(&|p| p.starts_with(&prefix)), !only_animated);
            }
        }
    }
    let z_only = |p: &str| matches!(p, "transform.position_z" | "transform.anchor_z" | "transform.rotation_x" | "transform.rotation_y");
    leaf(&mut out, "transform", "トランスフォーム", 1, true, None, pick(&|p| p.starts_with("transform.") && (three || !z_only(p))), false);
    if audio {
        leaf(&mut out, "audio", "オーディオ", 1, false, None, pick(&|p| p.starts_with("audio.")), false);
    }
    leaf(&mut out, "camera", "カメラオプション", 1, true, None, pick(&|p| p.starts_with("camera.")), false);
    leaf(&mut out, "light", "ライトオプション", 1, true, None, pick(&|p| p.starts_with("light.")), false);
    out
}

fn take_top(area: &mut Rect, h: f32) -> Rect {
    let r = Rect::from_min_max(area.min, pos2(area.right(), (area.top() + h).min(area.bottom())));
    area.min.y = r.bottom();
    r
}

fn take_bottom(area: &mut Rect, h: f32) -> Rect {
    let r = Rect::from_min_max(pos2(area.left(), (area.bottom() - h).max(area.top())), area.max);
    area.max.y = r.top();
    r
}

fn child(ui: &mut egui::Ui, rect: Rect, layout: Layout) -> egui::Ui {
    let mut c = ui.new_child(UiBuilder::new().max_rect(rect).layout(layout));
    c.set_clip_rect(rect.intersect(ui.clip_rect()));
    c
}

impl App {
    pub(crate) fn tl_is_open(&self, key: &str, default_open: bool) -> bool {
        default_open ^ self.tl_open.contains(key)
    }

    fn tl_toggle(&mut self, key: &str) {
        if !self.tl_open.remove(key) {
            self.tl_open.insert(key.to_string());
        }
    }

    pub(crate) fn timeline(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let shared = self.shared.clone();
        let Some(comp) = shared.comp(&self.comp_id) else { return };
        let total = comp.frame_count().max(1);
        let fr = comp.frame_rate;
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);

        let mut tab = 0;
        let rest = widgets::tab_strip(ui, &[&comp.name], &mut tab);
        ui.painter().text(
            rest.right_center() - vec2(8.0, 0.0),
            Align2::RIGHT_CENTER,
            format!("{} レイヤー　{}×{}　{:.2} fps", comp.layers.len(), comp.width, comp.height, fr.fps()),
            FontId::proportional(10.0),
            theme::TEXT_DIM,
        );

        let full = ui.available_rect_before_wrap();
        ui.allocate_rect(full, Sense::hover());
        let mut area = full;
        let head = take_top(&mut area, HEAD_H);
        let cols = take_top(&mut area, COLS_H);
        let foot = take_bottom(&mut area, FOOT_H);
        let audio = self.has_audio().then(|| take_top(&mut area, AUDIO_ROW));

        self.tl_left = self.tl_left.clamp(300.0, (full.width() - 240.0).max(300.0));
        let geo = Geo {
            x0: full.left(),
            split: full.left() + self.tl_left,
            show_parent: self.tl_left >= 440.0,
            show_matte: self.tl_left >= 370.0,
        };
        // 先頭フレームのキーフレームが欠けないよう、左に少し余白を取る
        let (time_l, time_r) = (geo.split + 8.0, full.right() - theme::SCROLL_W);
        let time_w = (time_r - time_l).max(1.0);
        let left_of = |r: Rect| Rect::from_min_max(r.min, pos2(geo.split, r.bottom()));
        let time_of = |r: Rect| Rect::from_min_max(pos2(time_l, r.top()), pos2(time_r, r.bottom()));

        // 表示範囲を決める（拡大率の上限は 1 フレームが MAX_PX_PER_FRAME になるまで）
        let max_zoom = (total as f64 * MAX_PX_PER_FRAME / time_w as f64).max(1.0);
        self.tl_zoom = self.tl_zoom.clamp(1.0, max_zoom);
        let span = total as f64 / self.tl_zoom;
        self.tl_scroll = self.tl_scroll.clamp(0.0, total as f64 - span);
        let view = TlView { left: time_l, width: time_w, start: self.tl_scroll, span };
        self.tl_view = Some(view);
        // (倍率, 中心にする画面 x)。ボタンやホイールからの拡大・縮小の要求
        let mut zoom_request: Option<(f64, Option<f32>)> = None;

        let bg = ui.painter_at(full);
        bg.rect_filled(left_of(head.union(cols)), 0.0, theme::BG2);
        bg.rect_filled(Rect::from_min_max(pos2(geo.split, head.top()), cols.right_bottom()), 0.0, theme::BG2);
        bg.rect_filled(left_of(area), 0.0, theme::BG1);
        bg.rect_filled(Rect::from_min_max(pos2(geo.split, area.top()), area.right_bottom()), 0.0, theme::BG0);
        bg.rect_filled(foot, 0.0, theme::BG2);

        self.tl_header(ui, comp, left_of(head), total);
        self.tl_columns(ui, left_of(cols), geo);
        self.tl_ruler(ui, comp, time_of(head), view, total);
        self.tl_marker_lane(ui, comp, &shared, time_of(cols), view, total);
        if let Some(r) = audio {
            self.tl_audio(ctx, ui, comp, left_of(r), time_of(r), view, total);
        }

        // レイヤーの行
        self.marquee_hits.clear();
        // 表示されていない行の位置は覚えておかない
        self.anchors.retain(|k, _| !k.starts_with("row:"));
        let search = self.tl_search.trim().to_lowercase();
        let t = fr.frame_to_time(self.frame);
        let mut rows_ui = child(ui, area, Layout::top_down(Align::Min));
        rows_ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        // 並び替えの落とし先を決めるための、(重ね順の位置, 行の上端) と最後の行の下端
        let mut blocks: Vec<(usize, f32)> = Vec::new();
        let mut rows_end = area.top();
        let reordering = matches!(self.tl_drag, Some(TlDrag::Reorder { .. }));
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .show(&mut rows_ui, |ui| {
                for (index, layer) in comp.layers.iter().enumerate() {
                    if (self.hide_shy && layer.shy) || (!search.is_empty() && !layer.name.to_lowercase().contains(&search)) {
                        continue;
                    }
                    let row = self.layer_row(ui, comp, layer, index + 1, view, geo);
                    blocks.push((index, row.top()));
                    if !self.tl_is_open(&layer.id.0, false) {
                        continue;
                    }
                    let rows = collect_rows(layer, t);
                    let three =
                        layer.three_d || matches!(layer.kind, LayerKind::Camera { .. } | LayerKind::Light { .. } | LayerKind::Model { .. });
                    let has_audio = znd_audio::layer_footage(&shared, layer).is_some();
                    let nodes = layer_tree(layer, &rows, three, has_audio, self.only_animated);
                    // 畳んだグループの配下は飛ばす
                    let mut closed: Option<usize> = None;
                    for node in &nodes {
                        if let Some(d) = closed {
                            if node.depth > d {
                                continue;
                            }
                            closed = None;
                        }
                        match &node.kind {
                            NodeKind::Group { key, label, default_open, times, fx } => {
                                let open = self.tl_is_open(key, *default_open);
                                self.group_row(ui, comp, layer, key, label, node.depth, open, times, *fx, view, geo);
                                if !open {
                                    closed = Some(node.depth);
                                }
                            }
                            NodeKind::Prop(i) => self.property_row(ui, comp, layer, &rows[*i], node.depth, view, geo),
                        }
                    }
                }
                rows_end = ui.cursor().top();
                // 並び替え中は、上下の端に近づくと行を送る
                if let Some(pos) = ui.input(|i| i.pointer.latest_pos()).filter(|_| reordering) {
                    let edge = 18.0;
                    let dy = if pos.y < area.top() + edge {
                        6.0
                    } else if pos.y > area.bottom() - edge {
                        -6.0
                    } else {
                        0.0
                    };
                    if dy != 0.0 {
                        ui.scroll_with_delta(vec2(0.0, dy));
                        ui.ctx().request_repaint();
                    }
                }
            });
        self.tl_reorder(ui, comp, left_of(area), &blocks, rows_end);
        self.tl_marquee(ui, Rect::from_min_max(pos2(time_l, area.top()), pos2(time_r, area.bottom())));

        // 現在時刻インジケーター（ルーラーから行の下端まで）
        let lanes = Rect::from_min_max(pos2(time_l, head.top() + WORK_H), pos2(time_r, area.bottom()));
        let p = ui.painter().with_clip_rect(lanes);
        let x = view.x(self.frame as f64).round() + 0.5;
        p.vline(x, cols.top()..=area.bottom(), Stroke::new(1.0_f32, theme::PLAYHEAD));
        let y = head.bottom() - 12.0;
        p.add(Shape::convex_polygon(
            vec![pos2(x - 5.0, y), pos2(x + 5.0, y), pos2(x + 5.0, y + 7.0), pos2(x, y + 12.0), pos2(x - 5.0, y + 7.0)],
            theme::PLAYHEAD,
            Stroke::NONE,
        ));

        // 境界線
        let p = ui.painter_at(full);
        p.hline(full.x_range(), cols.bottom() - 0.5, Stroke::new(1.0_f32, theme::LINE));
        p.hline(full.x_range(), foot.top() + 0.5, Stroke::new(1.0_f32, theme::LINE));
        p.vline(geo.split + 0.5, head.top()..=foot.bottom(), Stroke::new(1.0_f32, theme::LINE_HI));

        self.tl_footer(ui, left_of(foot), time_of(foot), view, total, max_zoom, &mut zoom_request);

        // 左右のペインの境界をドラッグして幅を変える（ダブルクリックで既定に戻す）
        let grip = Rect::from_min_max(pos2(geo.split - 3.0, head.top()), pos2(geo.split + 3.0, foot.bottom()));
        let resp = ui.interact(grip, ui.id().with("tl_split"), Sense::click_and_drag());
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
        }
        if resp.dragged() {
            self.tl_left += resp.drag_delta().x;
        }
        if resp.double_clicked() {
            self.tl_left = LEFT_DEFAULT;
        }

        // 拡大・縮小とスクロール（タイムラインの上にポインタがあるとき）
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let over = pointer.is_some_and(|p| p.x >= view.left && p.x <= view.left + view.width && full.contains(p));
        if over {
            let (scroll, zoom_delta, cmd, alt, shift) =
                ui.input(|i| (i.raw_scroll_delta, i.zoom_delta(), i.modifiers.command, i.modifiers.alt, i.modifiers.shift));
            if (zoom_delta - 1.0).abs() > 1e-3 {
                // ⌘ + ホイール、またはピンチ
                zoom_request = Some((zoom_delta as f64, pointer.map(|p| p.x)));
            } else if alt && scroll.y.abs() > 0.0 {
                zoom_request = Some(((scroll.y as f64 * 0.01).exp(), pointer.map(|p| p.x)));
            } else if !cmd {
                let dx = if scroll.x.abs() > 0.0 {
                    scroll.x
                } else if shift {
                    scroll.y
                } else {
                    0.0
                };
                self.tl_scroll -= dx as f64 / view.px_per_frame();
            }
        }
        if let Some((factor, at)) = zoom_request {
            // ポインタの下（ボタンからなら再生ヘッド）のフレームが動かないように拡大する
            let pivot = at.map(|x| view.frame(x)).unwrap_or(self.frame as f64);
            let ratio =
                at.map(|x| ((x - view.left) / view.width) as f64).unwrap_or_else(|| ((pivot - view.start) / view.span).clamp(0.0, 1.0));
            self.tl_zoom = (self.tl_zoom * factor).clamp(1.0, max_zoom);
            let span = total as f64 / self.tl_zoom;
            self.tl_scroll = (pivot - ratio * span).clamp(0.0, (total as f64 - span).max(0.0));
        }
        if ui.input(|i| i.pointer.any_released()) && self.tl_drag.is_some() {
            self.tl_drag = None;
            // ドラッグごとに取り消しの単位を分ける
            self.last_edit = None;
        }
    }

    /// 矩形選択: 行が拾ったキーで選択を置き換え、矩形を描く。
    fn tl_marquee(&mut self, ui: &egui::Ui, lanes: Rect) {
        let Some(TlDrag::Marquee { start, base }) = &self.tl_drag else { return };
        let Some(pos) = ui.input(|i| i.pointer.latest_pos()) else { return };
        let r = Rect::from_two_pos(*start, pos).intersect(lanes);
        let mut sel = base.clone();
        for k in self.marquee_hits.drain(..) {
            if !sel.contains(&k) {
                sel.push(k);
            }
        }
        self.selected_keys = sel;
        let p = ui.painter().with_clip_rect(lanes);
        p.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(120, 170, 255, 28));
        p.rect_stroke(r, 0.0, Stroke::new(1.0_f32, Color32::from_rgb(120, 170, 255)));
    }

    /// キーの並ぶ段に共通の操作。`times` は段に描いたキーの時刻、`prefix` はキーを拾うプロパティ（`None` で全部）。
    /// 矩形選択の当たり判定、キーを掴んだときのドラッグ開始（Alt で伸縮）、ドラッグ中の移動、クリックでの選択を行う。
    /// `marquee_from_empty` なら、キーの無いところから始めたドラッグを矩形選択にする。
    /// キーを掴んだ・矩形選択を始めたなど、この段がポインタ操作を受け持ったら true。
    #[allow(clippy::too_many_arguments)]
    fn key_lane(
        &mut self,
        ui: &egui::Ui,
        comp: &Composition,
        layer: &Layer,
        resp: &egui::Response,
        lane: Rect,
        times: &[Time],
        prefix: Option<&str>,
        view: TlView,
        marquee_from_empty: bool,
    ) -> bool {
        let fr = comp.frame_rate;
        let x_of = |t: Time| view.x(t.seconds() * fr.fps());
        let key_at = |px: f32| {
            times
                .iter()
                .copied()
                .filter(|t| (x_of(*t) - px).abs() <= GRAB)
                .min_by(|a, b| (x_of(*a) - px).abs().total_cmp(&(x_of(*b) - px).abs()))
        };
        // 矩形選択の当たり判定
        if let (Some(TlDrag::Marquee { start, .. }), Some(pos)) = (&self.tl_drag, ui.input(|i| i.pointer.latest_pos())) {
            let r = Rect::from_two_pos(*start, pos);
            if r.bottom() >= lane.top() && r.top() <= lane.bottom() {
                for t in times.iter().filter(|t| x_of(**t) >= r.left() - 2.0 && x_of(**t) <= r.right() + 2.0) {
                    let picked = crate::keyops::expand(layer, prefix, *t);
                    self.marquee_hits.extend(picked);
                }
            }
        }
        let (additive, alt) = ui.input(|i| (i.modifiers.shift || i.modifiers.command, i.modifiers.alt));
        let mut handled = false;
        if resp.drag_started() && !layer.locked && self.tl_drag.is_none() {
            if let Some(origin) = ui.input(|i| i.pointer.press_origin()) {
                if let Some(at) = key_at(origin.x) {
                    self.select_layer(&layer.id);
                    self.begin_key_drag(comp, crate::keyops::expand(layer, prefix, at), at, alt);
                    handled = true;
                } else if marquee_from_empty {
                    let base = if additive { self.selected_keys.clone() } else { Vec::new() };
                    self.tl_drag = Some(TlDrag::Marquee { start: origin, base });
                    handled = true;
                }
            }
        }
        if resp.dragged() && matches!(self.tl_drag, Some(TlDrag::Keys { .. })) {
            if let Some(pos) = resp.interact_pointer_pos() {
                // Alt は伸縮の開始に使うので、吸着の解除は Shift だけ
                let snap = self.snap && !ui.input(|i| i.modifiers.shift);
                self.drag_keys(comp, view, pos.x, snap);
                handled = true;
            }
        }
        let hit = resp.interact_pointer_pos().and_then(|p| key_at(p.x));
        if resp.clicked() {
            self.select_layer(&layer.id);
            let picked = hit.map(|at| crate::keyops::expand(layer, prefix, at)).unwrap_or_default();
            if hit.is_some() || !additive {
                self.click_keys(picked, additive);
            }
            handled |= hit.is_some();
        }
        if resp.secondary_clicked() {
            self.select_layer(&layer.id);
            // 選択に入っていないキーを右クリックしたら、それだけを選ぶ
            if let Some(at) = hit.filter(|at| !self.is_key_selected(&layer.id, prefix, *at)) {
                self.selected_keys = crate::keyops::expand(layer, prefix, at);
            }
        }
        handled
    }

    /// 再生ヘッドをポインタの位置へ（ルーラー・マーカーの段・波形で共通）。Shift を押している間はスナップしない。
    fn tl_scrub(&mut self, ui: &egui::Ui, resp: &egui::Response, view: TlView) {
        if resp.dragged() || resp.clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let free = ui.input(|i| i.modifiers.shift);
                self.set_frame(view.frame(pos.x).floor() as i64, !free);
            }
        }
    }

    /// 左上: 現在時刻、レイヤーの検索、表示の切り替え。
    fn tl_header(&mut self, ui: &mut egui::Ui, comp: &Composition, rect: Rect, total: i64) {
        let fr = comp.frame_rate;
        let mut ui = child(ui, rect.shrink2(vec2(6.0, 2.0)), Layout::left_to_right(Align::Center));
        ui.spacing_mut().item_spacing = vec2(4.0, 0.0);
        let mut f = self.frame;
        let changed = ui
            .scope(|ui| {
                ui.visuals_mut().override_text_color = Some(theme::VALUE);
                ui.visuals_mut().widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
                ui.style_mut().override_font_id = Some(FontId::monospace(15.0));
                ui.add(
                    egui::DragValue::new(&mut f)
                        .range(0..=total - 1)
                        .custom_formatter(move |v, _| timecode(fr, v as i64))
                        .custom_parser(|s| s.trim().parse::<f64>().ok()),
                )
                .on_hover_text("現在の時刻。ドラッグで移動、クリックしてフレーム番号を入力")
                .changed()
            })
            .inner;
        if changed {
            self.set_frame(f, false);
        }
        ui.label(egui::RichText::new(format!("{:05} ({:.2} fps)", self.frame, fr.fps())).size(10.0).color(theme::TEXT_DIM));
        ui.add_space(4.0);
        widgets::search_box(&mut ui, &mut self.tl_search, "レイヤーを検索", 96.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            if widgets::icon_toggle(
                ui,
                Icon::Magnet,
                self.snap,
                "スナップ: ビートグリッド・マーカー・再生ヘッド・レイヤーの端に吸着（Shift で一時解除）",
            )
            .clicked()
            {
                self.snap = !self.snap;
            }
            if widgets::icon_toggle(ui, Icon::Graph, self.show.graph, "グラフエディター").clicked() {
                self.show.graph = !self.show.graph;
            }
            if widgets::icon_toggle(
                ui,
                Icon::MotionBlur,
                comp.motion_blur.enabled,
                "モーションブラーを有効にする（スイッチを入れたレイヤーに掛かります）",
            )
            .clicked()
            {
                let on = !comp.motion_blur.enabled;
                self.edit_comp("モーションブラー", false, |c| c.motion_blur.enabled = on);
            }
            if widgets::icon_toggle(ui, Icon::Diamond, self.only_animated, "キーフレームかエクスプレッションのあるプロパティだけを表示")
                .clicked()
            {
                self.only_animated = !self.only_animated;
            }
            if widgets::icon_toggle(ui, Icon::Shy, self.hide_shy, "シャイのレイヤーを隠す").clicked() {
                self.hide_shy = !self.hide_shy;
            }
            if widgets::icon_button(ui, Icon::Marker, "現在の時刻にマーカーを追加 (M)").clicked() {
                self.add_marker("");
            }
        });
    }

    /// 列の見出し。
    fn tl_columns(&mut self, ui: &mut egui::Ui, rect: Rect, geo: Geo) {
        let p = ui.painter_at(rect);
        let dim = theme::TEXT_DIM;
        for (i, icon) in [Icon::Eye, Icon::Solo, Icon::Lock].into_iter().enumerate() {
            icons::paint_sized(&p, geo.cell(geo.x0 + 2.0 + i as f32 * AV_W, AV_W, rect), icon, dim, 12.0);
        }
        let font = FontId::proportional(10.0);
        let text = |x: f32, s: &str| {
            p.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, s, font.clone(), dim);
        };
        text(geo.x0 + 70.0, "#");
        text(geo.x0 + TREE_X + 14.0, "レイヤー名");
        for (i, icon) in [Icon::Shy, Icon::Fx, Icon::MotionBlur, Icon::Model].into_iter().enumerate() {
            icons::paint_sized(&p, geo.cell(geo.switches() + i as f32 * SW_W, SW_W, rect), icon, dim, 12.0);
        }
        text(geo.mode() + 5.0, "モード");
        if geo.show_matte {
            text(geo.matte() + 5.0, "マット");
        }
        if geo.show_parent {
            text(geo.parent() + 5.0, "親");
        }
        for x in [geo.x0 + 51.0, geo.switches() - 2.0, geo.mode() - 1.0] {
            p.vline(x, rect.y_range(), Stroke::new(1.0_f32, theme::LINE));
        }
    }

    /// ワークエリアのバーと、時間のルーラー。
    fn tl_ruler(&mut self, ui: &mut egui::Ui, comp: &Composition, rect: Rect, view: TlView, total: i64) {
        let fr = comp.frame_rate;
        let resp = ui.interact(rect, ui.id().with("ruler"), Sense::click_and_drag());
        self.anchors.insert("ruler".into(), rect);
        let p = ui.painter_at(rect);

        // ワークエリア
        let strip = Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + WORK_H));
        p.rect_filled(strip, 0.0, theme::INPUT);
        let (wa, wb) = self.work_area.unwrap_or((0, total));
        let (xa, xb) = (view.x(wa as f64), view.x(wb as f64));
        let bar = Rect::from_min_max(pos2(xa, strip.top() + 1.0), pos2(xb, strip.bottom() - 1.0));
        p.rect_filled(bar, 1.0, if self.work_area.is_some() { theme::ACCENT_HI } else { Color32::from_gray(0x4a) });
        for x in [xa + 1.5, xb - 1.5] {
            p.vline(x, bar.y_range(), Stroke::new(2.0_f32, Color32::from_gray(0xd8)));
        }

        // 目盛り（大・中・小）。秒の位置は 分:秒、それ以外はフレーム番号
        let (major, mid, minor) = tick_steps(fr.fps(), view.px_per_frame());
        let fps = (fr.fps().round() as i64).max(1);
        let (first, last) = ((view.start.floor() as i64).div_euclid(minor) * minor, (view.start + view.span).ceil() as i64);
        let mut f = first.max(0);
        while f <= last.min(total) {
            let x = view.x(f as f64).round() + 0.5;
            let (len, color) = if f % major == 0 {
                (10.0, Color32::from_gray(0x9a))
            } else if mid > 0 && f % mid == 0 {
                (6.0, Color32::from_gray(0x70))
            } else {
                (3.0, Color32::from_gray(0x55))
            };
            p.vline(x, (rect.bottom() - len)..=rect.bottom(), Stroke::new(1.0_f32, color));
            if f % major == 0 {
                let label = if f % fps == 0 { format!("{:02}:{:02}", f / fps / 60, f / fps % 60) } else { format!("{:02}f", f % fps) };
                p.text(pos2(x + 3.0, strip.bottom() + 1.0), Align2::LEFT_TOP, label, FontId::monospace(10.0), theme::TEXT_DIM);
            }
            f += minor;
        }
        self.tl_scrub(ui, &resp, view);

        // ワークエリアの操作: 両端を掴んで範囲を変える、中央を掴んで移動、ダブルクリックで解除
        let id = ui.id().with("work_area");
        let body = ui.interact(bar.expand2(vec2(0.0, 1.0)), id, Sense::click_and_drag());
        let start =
            ui.interact(Rect::from_min_max(pos2(xa - 3.0, strip.top()), pos2(xa + 5.0, strip.bottom())), id.with("s"), Sense::drag());
        let end = ui.interact(Rect::from_min_max(pos2(xb - 5.0, strip.top()), pos2(xb + 3.0, strip.bottom())), id.with("e"), Sense::drag());
        if start.hovered() || end.hovered() || start.dragged() || end.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        let frame_at = |r: &egui::Response| r.interact_pointer_pos().map(|pos| view.frame(pos.x).round() as i64);
        if let Some(f) = frame_at(&start).filter(|_| start.dragged()) {
            self.work_area = Some((f.clamp(0, wb - 1), wb));
        }
        if let Some(f) = frame_at(&end).filter(|_| end.dragged()) {
            self.work_area = Some((wa, f.clamp(wa + 1, total)));
        }
        if body.drag_started() {
            let grab = body.interact_pointer_pos().map(|pos| view.frame(pos.x) - wa as f64).unwrap_or(0.0);
            ui.data_mut(|d| d.insert_temp(id, grab));
        }
        if body.dragged() && self.work_area.is_some() {
            if let Some(pos) = body.interact_pointer_pos() {
                let grab: f64 = ui.data(|d| d.get_temp(id)).unwrap_or(0.0);
                let a = ((view.frame(pos.x) - grab).round() as i64).clamp(0, total - (wb - wa));
                self.work_area = Some((a, a + (wb - wa)));
            }
        }
        if body.double_clicked() {
            self.work_area = None;
        }
        body.on_hover_text(
            "ワークエリア（ループ再生と書き出しの範囲）。両端をドラッグで変更、B / N キーで現在の時刻に設定、ダブルクリックで解除",
        );
    }

    /// ルーラーの下の段: ビートグリッド、マーカー、レビューコメント、警告区間、キャッシュ済みのフレーム。
    fn tl_marker_lane(&mut self, ui: &mut egui::Ui, comp: &Composition, shared: &Project, rect: Rect, view: TlView, total: i64) {
        let fr = comp.frame_rate;
        let resp = ui.interact(rect, ui.id().with("marker_lane"), Sense::click_and_drag());
        let p = ui.painter_at(rect);
        let x_of = |t: Time| view.x(t.seconds() * fr.fps());
        let (first, last) = (view.start.floor() as i64, (view.start + view.span).ceil() as i64);

        // RAM プレビュー済みのフレーム
        for f in self.engine.cached_frames(total, self.downsample) {
            let x0 = view.x(f as f64);
            let x1 = view.x(f as f64 + 1.0).max(x0 + 1.0);
            p.rect_filled(Rect::from_min_max(pos2(x0, rect.top()), pos2(x1, rect.top() + 2.0)), 0.0, theme::CACHE);
        }
        // 光過敏性の警告区間
        for w in &self.check.flashes {
            let r = Rect::from_min_max(
                pos2(view.x(w.start_frame as f64), rect.top() + 2.0),
                pos2(view.x(w.end_frame as f64 + 1.0), rect.top() + 5.0),
            );
            p.rect_filled(r, 0.0, theme::ERROR);
        }
        // ビートグリッド（線が密になりすぎるときは拍頭・小節頭だけにする）
        if let Some(g) = &comp.beat_grid {
            let lines = g.lines(fr.frame_to_time(first.max(0)), fr.frame_to_time(last.min(total) + 1));
            let dense = lines.len() as f32 > rect.width() / 4.0;
            for (t, bar, beat) in lines {
                if dense && !beat {
                    continue;
                }
                let (len, col) = if bar {
                    (rect.height(), theme::BEAT)
                } else if beat {
                    (8.0, theme::BEAT.gamma_multiply(0.6))
                } else {
                    (4.0, Color32::from_gray(0x55))
                };
                p.vline(x_of(t).round() + 0.5, (rect.bottom() - len)..=rect.bottom(), Stroke::new(1.0_f32, col));
            }
        }
        for m in &comp.markers {
            let x = x_of(m.time);
            let tip = pos2(x, rect.bottom() - 2.0);
            p.add(Shape::convex_polygon(
                vec![tip, pos2(x - 4.0, tip.y - 5.0), pos2(x - 4.0, tip.y - 10.0), pos2(x + 4.0, tip.y - 10.0), pos2(x + 4.0, tip.y - 5.0)],
                theme::WARN,
                Stroke::NONE,
            ));
        }
        // レビューコメント
        for r in shared.reviews.iter().filter(|r| r.comp == comp.id && !r.resolved) {
            p.circle_filled(pos2(x_of(r.time), rect.center().y), 3.5, theme::VALUE);
        }

        if let Some(hover) = resp.hover_pos() {
            let tip = comp
                .markers
                .iter()
                .find(|m| (x_of(m.time) - hover.x).abs() < 5.0 && !m.comment.is_empty())
                .map(|m| m.comment.clone())
                .or_else(|| {
                    shared
                        .reviews
                        .iter()
                        .find(|r| r.comp == comp.id && !r.resolved && (x_of(r.time) - hover.x).abs() < 5.0)
                        .map(|r| format!("{}: {}", r.author, r.text))
                })
                .or_else(|| {
                    self.check
                        .flashes
                        .iter()
                        .find(|w| hover.x >= view.x(w.start_frame as f64) && hover.x <= view.x(w.end_frame as f64 + 1.0))
                        .map(|w| w.message())
                });
            if let Some(text) = tip {
                resp.clone().on_hover_text_at_pointer(text);
            }
        }
        self.tl_scrub(ui, &resp, view);
    }

    /// 音声: 波形またはスペクトログラム。
    #[allow(clippy::too_many_arguments)]
    fn tl_audio(&mut self, ctx: &egui::Context, ui: &mut egui::Ui, comp: &Composition, left: Rect, rect: Rect, view: TlView, total: i64) {
        let fr = comp.frame_rate;
        ui.painter_at(left).rect_filled(left, 0.0, theme::BG1);
        ui.painter_at(left.union(rect)).hline(left.left()..=rect.right(), rect.bottom() - 0.5, Stroke::new(1.0_f32, theme::LINE));
        {
            let mut ui = child(ui, left.shrink2(vec2(6.0, 2.0)), Layout::left_to_right(Align::Center));
            ui.spacing_mut().item_spacing = vec2(4.0, 0.0);
            let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
            icons::paint_sized(ui.painter(), r, Icon::Audio, theme::OK, 13.0);
            ui.label("オーディオ");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.selectable_value(&mut self.show_spectrogram, true, "スペクトログラム");
                ui.selectable_value(&mut self.show_spectrogram, false, "波形");
            });
        }

        let resp = ui.interact(rect, ui.id().with("wave"), Sense::click_and_drag());
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 0.0, theme::INPUT);
        let buf = self.audio_buf.clone();
        let frames = buf.len() / 2;
        self.anchors.insert("wave".into(), rect);
        // 表示範囲に対応するサンプル位置（小数）
        let samples_per_frame = SAMPLE_RATE as f64 / fr.fps();
        let (s0, s1) = (view.start * samples_per_frame, (view.start + view.span) * samples_per_frame);
        let wave_color = Color32::from_rgb(0x5a, 0xa8, 0x82);
        if self.show_spectrogram {
            let tex = self
                .spectrogram
                .get_or_insert_with(|| ctx.load_texture("spectrogram", spectrogram_image(&buf), egui::TextureOptions::LINEAR));
            let n = frames.max(1) as f64;
            let uv = Rect::from_min_max(pos2((s0 / n) as f32, 0.0), pos2((s1 / n) as f32, 1.0));
            p.image(tex.id(), rect, uv, Color32::WHITE);
        } else if frames > 0 {
            let mid = rect.center().y;
            let half = rect.height() / 2.0 - 2.0;
            let cols = rect.width().max(1.0) as usize;
            let per_px = (s1 - s0) / cols as f64;
            let value = |i: usize| (buf[i * 2] + buf[i * 2 + 1]) * 0.5;
            if per_px < 1.0 {
                // サンプル単位: 1 サンプルが 1px 以上あるので、標本点を結んだ折れ線で描く
                let (a, b) = (s0.floor().max(0.0) as usize, (s1.ceil() as usize + 1).min(frames));
                let x_of_sample = |i: usize| rect.left() + ((i as f64 - s0) / (s1 - s0)) as f32 * rect.width();
                let pts: Vec<egui::Pos2> = (a..b).map(|i| pos2(x_of_sample(i), mid - value(i) * half)).collect();
                if per_px < 0.25 {
                    for pt in &pts {
                        p.circle_filled(*pt, 2.0, wave_color);
                    }
                }
                p.add(Shape::line(pts, Stroke::new(1.0_f32, wave_color)));
                p.line_segment([pos2(rect.left(), mid), pos2(rect.right(), mid)], Stroke::new(1.0_f32, theme::LINE));
            } else {
                // 1 画素ぶんの区間の最小・最大を縦線で描く
                for c in 0..cols {
                    let a = (s0 + c as f64 * per_px).max(0.0) as usize;
                    let b = ((s0 + (c + 1) as f64 * per_px) as usize).max(a + 1);
                    let (mut lo, mut hi) = (0.0f32, 0.0f32);
                    // 長い区間は間引いて調べる（描画コストを一定に保つ）
                    let stride = ((b - a) / 64).max(1);
                    let mut i = a;
                    while i < b.min(frames) {
                        let v = value(i);
                        lo = lo.min(v);
                        hi = hi.max(v);
                        i += stride;
                    }
                    let x = rect.left() + c as f32;
                    p.line_segment([pos2(x, mid - hi * half), pos2(x, mid - lo * half + 1.0)], Stroke::new(1.0_f32, wave_color));
                }
            }
        }
        if let Some(g) = &comp.beat_grid {
            let (from, to) = (
                fr.frame_to_time((view.start.floor() as i64).max(0)),
                fr.frame_to_time(((view.start + view.span).ceil() as i64).min(total) + 1),
            );
            for (t, bar, beat) in g.lines(from, to) {
                if beat {
                    let x = view.x(t.seconds() * fr.fps());
                    p.vline(x, rect.y_range(), Stroke::new(1.0_f32, theme::BEAT.gamma_multiply(if bar { 0.45 } else { 0.16 })));
                }
            }
        }
        self.tl_scrub(ui, &resp, view);
    }

    /// 下端: 時間方向の拡大率と、横スクロールバー。
    #[allow(clippy::too_many_arguments)]
    fn tl_footer(
        &mut self,
        ui: &mut egui::Ui,
        left: Rect,
        track: Rect,
        view: TlView,
        total: i64,
        max_zoom: f64,
        zoom: &mut Option<(f64, Option<f32>)>,
    ) {
        {
            let mut ui = child(ui, left.shrink2(vec2(6.0, 0.0)), Layout::left_to_right(Align::Center));
            ui.spacing_mut().item_spacing = vec2(3.0, 0.0);
            ui.spacing_mut().interact_size.y = 13.0;
            ui.spacing_mut().slider_width = 110.0;
            let small = |ui: &mut egui::Ui, icon: Icon, tip: &str| {
                let (r, resp) = ui.allocate_exact_size(vec2(14.0, 13.0), Sense::click());
                icons::paint_sized(ui.painter(), r, icon, if resp.hovered() { Color32::WHITE } else { theme::TEXT_DIM }, 11.0);
                resp.on_hover_text(tip).clicked()
            };
            if small(&mut ui, Icon::Minus, "時間方向に縮小（⌘ または Alt + ホイール）") {
                *zoom = Some((0.5, None));
            }
            // 対数で動くつまみ（左端 = コンポ全体、右端 = 1 サンプル単位）
            let range = max_zoom.ln().max(1e-6);
            let mut v = (self.tl_zoom.ln() / range) as f32;
            if ui.add(egui::Slider::new(&mut v, 0.0..=1.0).show_value(false)).on_hover_text("タイムラインの拡大率").changed() {
                *zoom = Some(((v as f64 * range).exp() / self.tl_zoom, None));
            }
            if small(&mut ui, Icon::Plus, "時間方向に拡大（⌘ または Alt + ホイール）") {
                *zoom = Some((2.0, None));
            }
            if small(&mut ui, Icon::Fit, "コンポ全体を表示") {
                self.tl_zoom = 1.0;
                self.tl_scroll = 0.0;
            }
            ui.label(egui::RichText::new(format!("×{:.1}", self.tl_zoom)).size(10.0).color(theme::TEXT_DIM));
        }

        // 横スクロールバー
        let p = ui.painter_at(track);
        let lane = track.shrink2(vec2(0.0, 3.0));
        p.rect_filled(lane, 1.0, theme::INPUT);
        let (a, b) = ((view.start / total as f64) as f32, ((view.start + view.span) / total as f64) as f32);
        let thumb = Rect::from_min_max(
            pos2(lane.left() + a * lane.width(), lane.top()),
            pos2((lane.left() + b * lane.width()).max(lane.left() + a * lane.width() + 12.0), lane.bottom()),
        );
        let resp = ui.interact(track, ui.id().with("tl_hscroll"), Sense::click_and_drag());
        p.rect_filled(thumb, 1.0, if resp.dragged() || resp.hovered() { Color32::from_gray(0x5c) } else { Color32::from_gray(0x46) });
        if resp.dragged() {
            self.tl_scroll += (resp.drag_delta().x / lane.width()) as f64 * total as f64;
        } else if resp.clicked() {
            // つまみの外をクリックしたら、そこが中央に来るように移動する
            if let Some(pos) = resp.interact_pointer_pos().filter(|pos| !thumb.contains(*pos)) {
                self.tl_scroll = ((pos.x - lane.left()) / lane.width()) as f64 * total as f64 - view.span / 2.0;
            }
        }
    }

    /// ポインタの位置 `x` を時刻にする。`snap` なら、ビートグリッド・マーカー・再生ヘッド・他のレイヤーの端に吸着する。
    pub(crate) fn pointer_time(&self, comp: &Composition, view: TlView, x: f32, exclude: &LayerId, snap: bool) -> Time {
        let fr = comp.frame_rate;
        let t = Time::from_seconds(view.frame(x) / fr.fps());
        if !snap {
            return beat::nearest_frame(t, fr);
        }
        // 画面上で 6px 以内なら吸着
        let tolerance = Time::from_seconds(6.0 / view.px_per_frame() / fr.fps());
        edit::snap_time(comp, t, Some(exclude), &[fr.frame_to_time(self.frame)], tolerance)
    }

    pub(crate) fn select_layer(&mut self, id: &LayerId) {
        self.selected = Some(id.clone());
        self.selected_footage = None;
    }

    /// レイヤーを複製して、元のすぐ上に置く。複製したほうを選択する。
    pub(crate) fn duplicate_layer(&mut self, id: &LayerId) {
        let new_id = LayerId::new(self.shared.fresh_id("layer"));
        let (source, created) = (id.clone(), new_id.clone());
        self.edit_comp("レイヤー複製", false, |c| {
            edit::duplicate_layer(c, &source, new_id);
        });
        if self.shared.comp(&self.comp_id).is_some_and(|c| c.layer(&created).is_some()) {
            self.select_layer(&created);
            self.selected_keys.clear();
        }
    }

    /// レイヤーを削除する。ロック中のレイヤーは消さない。
    pub(crate) fn delete_layer(&mut self, id: &LayerId) {
        let shared = self.shared.clone();
        let Some(layer) = shared.comp(&self.comp_id).and_then(|c| c.layer(id)) else { return };
        if layer.locked {
            self.status = format!("「{}」はロックされているため削除できません", layer.name);
            return;
        }
        let target = id.clone();
        self.edit_comp("レイヤー削除", false, |c| {
            edit::remove_layer(c, &target);
        });
        // 他の人がロックしているコンポでは消えていない
        if self.shared.comp(&self.comp_id).is_some_and(|c| c.layer(id).is_some()) {
            return;
        }
        if self.selected.as_ref() == Some(id) {
            self.selected = None;
        }
        self.selected_keys.retain(|k| &k.layer != id);
        self.status = format!("「{}」を削除しました（⌘Z で元に戻せます）", layer.name);
    }

    /// レイヤーを重ね順の `to` 番目の手前へ移す（`to` は移動前の並びでの位置）。
    pub(crate) fn reorder_layer(&mut self, id: &LayerId, to: usize) {
        let moves = self.shared.comp(&self.comp_id).and_then(|c| c.layer_index(id)).is_some_and(|from| to != from && to != from + 1);
        if moves {
            let target = id.clone();
            self.edit_comp("重ね順", false, |c| {
                edit::reorder_layer(c, &target, to);
            });
        }
    }

    /// 行のドラッグによる並び替え。落とし先の位置に線を引き、ボタンを離したら移す（Esc で中止）。
    fn tl_reorder(&mut self, ui: &egui::Ui, comp: &Composition, left: Rect, blocks: &[(usize, f32)], rows_end: f32) {
        let Some(TlDrag::Reorder { layer }) = &self.tl_drag else { return };
        let layer = layer.clone();
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.tl_drag = None;
            return;
        }
        let Some(pos) = ui.input(|i| i.pointer.latest_pos()) else { return };
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        // ポインタが行の上半分ならその行の手前、下半分ならその行（と展開したプロパティ）の後ろ
        let mut slot: Option<(usize, f32)> = None;
        for (k, (index, top)) in blocks.iter().enumerate() {
            if pos.y < top + ROW * 0.5 {
                slot = Some((*index, *top));
                break;
            }
            slot = Some((index + 1, blocks.get(k + 1).map(|b| b.1).unwrap_or(rows_end)));
        }
        let Some((to, y)) = slot else { return };
        let moves = comp.layer_index(&layer).is_some_and(|from| to != from && to != from + 1);
        if moves {
            let p = ui.painter().with_clip_rect(left.expand2(vec2(0.0, 1.0)));
            let y = y.round() - 0.5;
            p.hline(left.x_range(), y, Stroke::new(2.0_f32, theme::PLAYHEAD));
            p.circle_filled(pos2(left.left() + 4.0, y), 3.0, theme::PLAYHEAD);
        }
        if ui.input(|i| i.pointer.any_released()) && moves {
            self.reorder_layer(&layer, to);
        }
    }

    /// レイヤーの行・バーを右クリックしたときのメニュー。
    fn layer_context_menu(&mut self, ui: &mut egui::Ui, comp: &Composition, layer: &Layer) {
        let id = &layer.id;
        let Some(index) = comp.layer_index(id) else {
            ui.close_menu();
            return;
        };
        let last = comp.layers.len() - 1;
        ui.set_min_width(170.0);
        ui.label(egui::RichText::new(&layer.name).size(10.0).color(theme::TEXT_DIM));
        ui.separator();
        if ui.add(egui::Button::new("複製").shortcut_text(self.shortcut_text(Action::Duplicate))).clicked() {
            ui.close_menu();
            self.duplicate_layer(id);
        }
        if ui.add_enabled(!layer.locked, egui::Button::new("再生ヘッドで分割").shortcut_text(self.shortcut_text(Action::SplitLayer))).clicked() {
            ui.close_menu();
            self.selected = Some(id.clone());
            self.run_action(Action::SplitLayer);
        }
        ui.separator();
        for (label, enabled, to) in [
            ("最前面へ", index > 0, 0),
            ("ひとつ前面へ", index > 0, index.saturating_sub(1)),
            ("ひとつ背面へ", index < last, index + 2),
            ("最背面へ", index < last, last + 1),
        ] {
            if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                ui.close_menu();
                self.reorder_layer(id, to);
            }
        }
        ui.menu_button("コマ打ち", |ui| {
            for n in 1..=4u32 {
                let label = if n == 1 { "なし（1 コマ）".to_string() } else { format!("{n} コマ打ち") };
                let action = [Action::Hold1, Action::Hold2, Action::Hold3, Action::Hold4][n as usize - 1];
                let shortcut = self.shortcut_text(action);
                if ui.add(egui::SelectableLabel::new(layer.hold_frames == n, label)).on_hover_text(shortcut).clicked() {
                    ui.close_menu();
                    self.edit_layer("コマ打ち", false, id, |l| l.hold_frames = n);
                }
            }
        });
        ui.separator();
        if ui.button(if layer.locked { "ロックを解除" } else { "ロック" }).clicked() {
            ui.close_menu();
            let v = !layer.locked;
            self.edit_layer("ロック", false, id, |l| l.locked = v);
        }
        if ui.button(if layer.visible { "非表示にする" } else { "表示する" }).clicked() {
            ui.close_menu();
            let v = !layer.visible;
            self.edit_layer("表示", false, id, |l| l.visible = v);
        }
        ui.separator();
        if ui.add_enabled(!layer.locked, egui::Button::new("削除").shortcut_text(self.shortcut_text(Action::Delete))).clicked() {
            ui.close_menu();
            self.delete_layer(id);
        }
        self.plugin_layer_menu(ui, id);
    }

    /// 行の背景と下端の線。選択中のレイヤーの行は少し明るくする。
    fn row_background(&self, ui: &egui::Ui, rect: Rect, geo: Geo, selected: bool) {
        let p = ui.painter_at(rect);
        let left = Rect::from_min_max(rect.min, pos2(geo.split, rect.bottom()));
        let time = Rect::from_min_max(pos2(geo.split, rect.top()), rect.max);
        p.rect_filled(left, 0.0, if selected { theme::ROW_SELECTED } else { theme::BG1 });
        p.rect_filled(time, 0.0, if selected { Color32::from_gray(0x24) } else { theme::BG0 });
        p.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0_f32, Color32::from_gray(0x29)));
    }

    #[allow(clippy::too_many_arguments)]
    fn layer_row(&mut self, ui: &mut egui::Ui, comp: &Composition, layer: &Layer, number: usize, view: TlView, geo: Geo) -> Rect {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
        if !ui.is_rect_visible(rect) {
            return rect;
        }
        let fr = comp.frame_rate;
        let id = layer.id.clone();
        let rid = ui.id().with(("layer", &id.0));
        let selected = self.selected.as_ref() == Some(&id);
        let color = theme::label_color(layer.label, &id.0);
        self.row_background(ui, rect, geo, selected);

        // ---------- 左: レイヤーの列 ----------
        let left = Rect::from_min_max(rect.min, pos2(geo.split, rect.bottom()));
        // クリックで選択、上下にドラッグで重ね順の並び替え、右クリックでメニュー
        let row = ui.interact(left, rid.with("row"), Sense::click_and_drag());
        if row.clicked() || row.secondary_clicked() {
            self.select_layer(&id);
        }
        if row.drag_started_by(egui::PointerButton::Primary) && self.tl_drag.is_none() {
            self.select_layer(&id);
            self.tl_drag = Some(TlDrag::Reorder { layer: id.clone() });
        }
        row.context_menu(|ui| self.layer_context_menu(ui, comp, layer));
        if matches!(&self.tl_drag, Some(TlDrag::Reorder { layer: l }) if *l == id) {
            ui.painter_at(left).rect_filled(left, 0.0, theme::ACCENT.gamma_multiply(0.35));
        }
        let off = theme::TEXT_OFF.gamma_multiply(0.6);
        let av = |i: usize| geo.cell(geo.x0 + 2.0 + i as f32 * AV_W, AV_W, rect);
        if widgets::icon_cell(ui, av(0), rid.with("eye"), Icon::Eye, layer.visible, off, "表示").clicked() {
            let v = !layer.visible;
            self.edit_layer("表示", false, &id, |l| l.visible = v);
        }
        if widgets::icon_cell(ui, av(1), rid.with("solo"), Icon::Solo, layer.solo, off, "ソロ").clicked() {
            let v = !layer.solo;
            self.edit_layer("ソロ", false, &id, |l| l.solo = v);
        }
        if widgets::icon_cell(ui, av(2), rid.with("lock"), Icon::Lock, layer.locked, off, "ロック").clicked() {
            let v = !layer.locked;
            self.edit_layer("ロック", false, &id, |l| l.locked = v);
        }

        // ラベルの色（クリックで変更）
        let p = ui.painter_at(left);
        let swatch = Rect::from_center_size(pos2(geo.x0 + 59.0, rect.center().y), vec2(10.0, 10.0));
        p.rect_filled(swatch, 1.0, color);
        let resp = ui.interact(swatch.expand(3.0), rid.with("label"), Sense::click()).on_hover_text("ラベルの色");
        let popup = rid.with("label_popup");
        if resp.clicked() {
            ui.memory_mut(|m| m.toggle_popup(popup));
        }
        egui::popup::popup_below_widget(ui, popup, &resp, egui::PopupCloseBehavior::CloseOnClick, |ui| {
            ui.set_min_width(120.0);
            for (i, (name, col)) in theme::LABELS.iter().enumerate() {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                    ui.painter().rect_filled(r, 1.0, *col);
                    if ui.selectable_label(layer.label as usize == i + 1, *name).clicked() {
                        self.edit_layer("ラベル", false, &id, |l| l.label = i as u8 + 1);
                    }
                });
            }
        });
        p.text(pos2(geo.x0 + 84.0, rect.center().y), Align2::RIGHT_CENTER, number.to_string(), FontId::proportional(10.5), theme::TEXT_DIM);

        let open = self.tl_is_open(&id.0, false);
        let twirl = geo.cell(geo.x0 + TREE_X, 12.0, rect);
        if widgets::icon_cell(
            ui,
            twirl,
            rid.with("twirl"),
            if open { Icon::TwirlOpen } else { Icon::TwirlClosed },
            false,
            theme::TEXT_DIM,
            "プロパティを展開",
        )
        .clicked()
        {
            self.tl_toggle(&id.0);
        }
        let dim = if layer.visible { 1.0 } else { 0.5 };
        icons::paint_sized(
            &p,
            geo.cell(geo.x0 + TREE_X + 13.0, 14.0, rect),
            kind_icon(&layer.kind),
            color.gamma_multiply(1.25 * dim),
            12.0,
        );
        let name_clip = Rect::from_min_max(pos2(geo.x0 + TREE_X + 30.0, rect.top()), pos2(geo.switches() - 4.0, rect.bottom()));
        self.anchors.insert(format!("row:name:{}", id.0), name_clip);
        p.with_clip_rect(name_clip).text(
            pos2(name_clip.left(), rect.center().y),
            Align2::LEFT_CENTER,
            &layer.name,
            FontId::proportional(11.5),
            if selected { Color32::WHITE } else { theme::TEXT.gamma_multiply(dim) },
        );

        // スイッチ
        let sw = |i: usize| geo.cell(geo.switches() + i as f32 * SW_W, SW_W, rect);
        if widgets::icon_cell(ui, sw(0), rid.with("shy"), Icon::Shy, layer.shy, off, "シャイ").clicked() {
            let v = !layer.shy;
            self.edit_layer("シャイ", false, &id, |l| l.shy = v);
        }
        if !layer.effects.is_empty() {
            let any = layer.effects.iter().any(|fx| fx.enabled);
            if widgets::icon_cell(ui, sw(1), rid.with("fx"), Icon::Fx, any, off, "エフェクトの有効／無効").clicked() {
                self.edit_layer("エフェクト有効", false, &id, |l| l.effects.iter_mut().for_each(|fx| fx.enabled = !any));
            }
        }
        if widgets::icon_cell(ui, sw(2), rid.with("mb"), Icon::MotionBlur, layer.motion_blur, off, "モーションブラー").clicked() {
            let v = !layer.motion_blur;
            self.edit_layer("モーションブラー", false, &id, |l| l.motion_blur = v);
        }
        if widgets::icon_cell(ui, sw(3), rid.with("3d"), Icon::Model, layer.three_d, off, "3D レイヤー").clicked() {
            let v = !layer.three_d;
            self.edit_layer("3D", false, &id, |l| l.three_d = v);
        }

        // モード・トラックマット・親
        widgets::cell_menu(ui, geo.cell(geo.mode(), MODE_W, rect), rid.with("mode"), layer.blend_mode.label_ja(), |ui| {
            for mode in BlendMode::ALL {
                if ui.selectable_label(mode == layer.blend_mode, mode.label_ja()).clicked() {
                    self.edit_layer("描画モード", false, &id, |l| l.blend_mode = mode);
                }
            }
        });
        let name_of = |lid: &LayerId| comp.layer(lid).map(|l| l.name.clone()).unwrap_or_else(|| "（不明）".into());
        if geo.show_matte {
            let text = layer.track_matte.as_ref().map(|m| name_of(&m.layer)).unwrap_or_else(|| "なし".into());
            widgets::cell_menu(ui, geo.cell(geo.matte(), MATTE_W, rect), rid.with("matte"), &text, |ui| {
                if ui.selectable_label(layer.track_matte.is_none(), "なし").clicked() {
                    self.edit_layer("トラックマット", false, &id, |l| l.track_matte = None);
                }
                for o in comp.layers.iter().filter(|l| l.id != id) {
                    if ui.selectable_label(layer.track_matte.as_ref().is_some_and(|m| m.layer == o.id), &o.name).clicked() {
                        let mode = layer.track_matte.as_ref().map(|m| m.mode).unwrap_or(MatteMode::Alpha);
                        let mid = o.id.clone();
                        self.edit_layer("トラックマット", false, &id, |l| l.track_matte = Some(TrackMatte { layer: mid, mode }));
                    }
                }
            });
        }
        if geo.show_parent {
            let text = layer.parent.as_ref().map(&name_of).unwrap_or_else(|| "なし".into());
            widgets::cell_menu(ui, geo.cell(geo.parent(), PARENT_W, rect), rid.with("parent"), &text, |ui| {
                if ui.selectable_label(layer.parent.is_none(), "なし").clicked() {
                    self.set_parent(&id, None);
                }
                for o in comp.layers.iter().filter(|l| l.id != id) {
                    if ui.selectable_label(layer.parent.as_ref() == Some(&o.id), &o.name).clicked() {
                        self.set_parent(&id, Some(o.id.clone()));
                    }
                }
            });
        }

        // ---------- 右: レイヤーのバー ----------
        let time = Rect::from_min_max(pos2(view.left, rect.top()), pos2(view.left + view.width, rect.bottom()));
        let resp = ui.interact(time, rid.with("bar"), Sense::click_and_drag());
        self.anchors.insert(format!("row:{}", id.0), time);
        let p = ui.painter_at(time);
        let x_of = |t: Time| view.x(t.seconds() * fr.fps());
        let (x0, x1) = (x_of(layer.in_point), x_of(layer.out_point));
        let bar = Rect::from_min_max(pos2(x0, rect.top() + 2.0), pos2(x1, rect.bottom() - 3.0));
        let fill = if layer.visible { color } else { color.gamma_multiply(0.4) };
        p.rect_filled(bar, 1.0, fill.gamma_multiply(if selected { 1.0 } else { 0.82 }));
        // イン点・アウト点の取っ手
        let grip = Color32::from_white_alpha(if selected { 110 } else { 60 });
        p.rect_filled(Rect::from_min_max(bar.min, pos2((bar.left() + 3.0).min(bar.right()), bar.bottom())), 0.0, grip);
        p.rect_filled(Rect::from_min_max(pos2((bar.right() - 3.0).max(bar.left()), bar.top()), bar.max), 0.0, grip);
        if selected {
            p.rect_stroke(bar, 1.0, Stroke::new(1.0_f32, Color32::from_white_alpha(150)));
        }
        // 畳んでいる間は、キーフレームのある時刻を小さな点で示す（掴むと、その時刻の全プロパティのキーが動く）
        let keys = if open { Vec::new() } else { key_times(layer) };
        for t in &keys {
            let picked = self.is_key_selected(&id, None, *t);
            let c = pos2(x_of(*t), bar.center().y);
            p.circle(
                c,
                2.6,
                if picked { theme::KEY_SELECTED } else { Color32::from_gray(0xe6) },
                Stroke::new(1.0_f32, Color32::from_black_alpha(160)),
            );
        }
        for m in &layer.markers {
            p.vline(x_of(m.time), (rect.top() + 2.0)..=(rect.bottom() - 3.0), Stroke::new(1.5_f32, theme::WARN));
        }

        // ポインタの下にあるもの: キーフレーム > バーの端 > バー本体
        let key_at = |x: f32| {
            keys.iter()
                .copied()
                .filter(|t| (x_of(*t) - x).abs() <= GRAB)
                .min_by(|a, b| (x_of(*a) - x).abs().total_cmp(&(x_of(*b) - x).abs()))
        };
        let locked = layer.locked;
        if let Some(hover) = resp.hover_pos().filter(|_| !locked && self.tl_drag.is_none()) {
            let icon = if key_at(hover.x).is_some() {
                Some(egui::CursorIcon::PointingHand)
            } else if (hover.x - x0).abs() <= GRAB || (hover.x - x1).abs() <= GRAB {
                Some(egui::CursorIcon::ResizeHorizontal)
            } else if hover.x > x0 && hover.x < x1 {
                Some(egui::CursorIcon::Grab)
            } else {
                None
            };
            if let Some(icon) = icon {
                ui.ctx().set_cursor_icon(icon);
            }
        }
        // バーの端にキーフレームが重なっているときは、バーの外側を掴めばトリム、内側ならキーフレーム
        let origin = ui.input(|i| i.pointer.press_origin());
        let outside_edge = origin.is_some_and(|o| {
            let x = o.x;
            (x <= x0 && x0 - x <= GRAB) || (x >= x1 && x - x1 <= GRAB)
        });
        let off_bar = origin.is_some_and(|o| o.x < x0 - GRAB || o.x > x1 + GRAB);
        // キーフレームの操作と矩形選択（バーの外から始めたとき）
        let lane_keys = if outside_edge && resp.drag_started() { Vec::new() } else { keys.clone() };
        let by_lane = self.key_lane(ui, comp, layer, &resp, time, &lane_keys, None, view, off_bar);
        if resp.drag_started() && !locked && !by_lane && self.tl_drag.is_none() {
            if let Some(origin) = origin {
                let x = origin.x;
                self.select_layer(&id);
                self.tl_drag = if (x - x0).abs() <= GRAB {
                    Some(TlDrag::TrimIn { layer: id.clone() })
                } else if (x - x1).abs() <= GRAB {
                    Some(TlDrag::TrimOut { layer: id.clone() })
                } else if x > x0 && x < x1 {
                    let grab = Time::from_seconds(view.frame(x) / fr.fps()) - layer.in_point;
                    Some(TlDrag::Move { layer: id.clone(), grab })
                } else {
                    None
                };
            }
        }
        if resp.dragged() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let snap = self.snap && !ui.input(|i| i.modifiers.shift);
                let mut drag = self.tl_drag.take();
                match &mut drag {
                    Some(TlDrag::Move { layer: lid, grab }) => {
                        // イン点・アウト点のどちらか近いほうを吸着させる
                        let raw_in = Time::from_seconds(view.frame(pos.x) / fr.fps()) - *grab;
                        let len = layer.out_point - layer.in_point;
                        let in_x = view.x(raw_in.seconds() * fr.fps());
                        let out_x = view.x((raw_in + len).seconds() * fr.fps());
                        let snapped_in = self.pointer_time(comp, view, in_x, lid, snap);
                        let snapped_out = self.pointer_time(comp, view, out_x, lid, snap);
                        let round = beat::nearest_frame(raw_in, fr);
                        let (d_in, d_out) = ((snapped_in - round).0.abs(), (snapped_out - beat::nearest_frame(raw_in + len, fr)).0.abs());
                        let target = if d_out != 0 && (d_in == 0 || d_out < d_in) { snapped_out - len } else { snapped_in };
                        let dt = target - layer.in_point;
                        if dt != Time::ZERO {
                            self.edit_layer("レイヤーを移動", true, lid, |l| edit::shift_layer(l, dt));
                            // 一緒に動いたキーの選択も追従させる
                            for k in self.selected_keys.iter_mut().filter(|k| k.layer == *lid) {
                                k.at += dt;
                            }
                        }
                    }
                    Some(TlDrag::TrimIn { layer: lid }) => {
                        let t = self.pointer_time(comp, view, pos.x, lid, snap);
                        if t != layer.in_point {
                            self.edit_layer("イン点を変更", true, lid, |l| edit::trim_in(l, t, fr));
                        }
                    }
                    Some(TlDrag::TrimOut { layer: lid }) => {
                        let t = self.pointer_time(comp, view, pos.x, lid, snap);
                        if t != layer.out_point {
                            self.edit_layer("アウト点を変更", true, lid, |l| edit::trim_out(l, t, fr));
                        }
                    }
                    _ => {}
                }
                self.tl_drag = drag;
            }
        }
        let on_selected_key = resp.interact_pointer_pos().and_then(|p| key_at(p.x)).is_some_and(|t| self.is_key_selected(&id, None, t));
        if resp.secondary_clicked() {
            ui.data_mut(|d| d.insert_temp(rid.with("menu-keys"), on_selected_key));
        }
        let menu_keys: bool = ui.data(|d| d.get_temp(rid.with("menu-keys"))).unwrap_or(false);
        if menu_keys {
            resp.context_menu(|ui| self.key_menu(ui));
            return rect;
        }
        resp.context_menu(|ui| self.layer_context_menu(ui, comp, layer));
        rect
    }

    /// グループの行（トランスフォーム・エフェクトなど）。
    #[allow(clippy::too_many_arguments)]
    fn group_row(
        &mut self,
        ui: &mut egui::Ui,
        comp: &Composition,
        layer: &Layer,
        key: &str,
        label: &str,
        depth: usize,
        open: bool,
        times: &[Time],
        fx: Option<usize>,
        view: TlView,
        geo: Geo,
    ) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
        if !ui.is_rect_visible(rect) {
            return;
        }
        let id = layer.id.clone();
        let rid = ui.id().with(("group", key));
        self.row_background(ui, rect, geo, false);
        let left = Rect::from_min_max(rect.min, pos2(geo.split, rect.bottom()));
        let x = geo.x0 + TREE_X + depth as f32 * 12.0;
        if ui.interact(left, rid, Sense::click()).clicked() {
            self.tl_toggle(key);
        }
        let p = ui.painter_at(left);
        icons::paint_sized(&p, geo.cell(x, 12.0, rect), if open { Icon::TwirlOpen } else { Icon::TwirlClosed }, theme::TEXT_DIM, 12.0);
        let mut color = theme::TEXT;
        if let Some(i) = fx {
            let on = layer.effects.get(i).is_some_and(|f| f.enabled);
            if !on {
                color = theme::TEXT_DIM;
            }
            if widgets::icon_cell(
                ui,
                geo.cell(geo.x0 + 2.0, AV_W, rect),
                rid.with("fx"),
                Icon::Fx,
                on,
                theme::TEXT_OFF,
                "このエフェクトの有効／無効",
            )
            .clicked()
            {
                self.edit_layer("エフェクト有効", false, &id, |l| {
                    if let Some(f) = l.effects.get_mut(i) {
                        f.enabled = !on;
                    }
                });
            }
        }
        p.text(pos2(x + 15.0, rect.center().y), Align2::LEFT_CENTER, label, FontId::proportional(11.0), color);

        // 畳んでいるグループは、配下のキーフレームの時刻を点で示す（掴むと配下のキーがまとめて動く）
        let fr = comp.frame_rate;
        let time = Rect::from_min_max(pos2(view.left, rect.top()), pos2(view.left + view.width, rect.bottom()));
        let resp = ui.interact(time, rid.with("keys"), Sense::click_and_drag());
        let prefix = key.split_once('/').map(|(_, p)| p).unwrap_or(key);
        let shown: &[Time] = if open { &[] } else { times };
        if !open {
            // 先頭フレームのキーフレームが欠けないよう、左の余白まで描く
            let p = ui.painter_at(time.expand2(vec2(6.0, 0.0)));
            for t in shown {
                let picked = self.is_key_selected(&id, Some(prefix), *t);
                p.circle_filled(
                    pos2(view.x(t.seconds() * fr.fps()), rect.center().y),
                    if picked { 2.8 } else { 2.2 },
                    if picked { theme::KEY_SELECTED } else { Color32::from_gray(0x9a) },
                );
            }
        }
        self.key_lane(ui, comp, layer, &resp, time, shown, Some(prefix), view, true);
        if self.selected_keys.iter().any(|k| k.layer == id && keys::path_matches(&k.path, prefix)) {
            resp.context_menu(|ui| self.key_menu(ui));
        }
    }

    /// プロパティ 1 行。左にストップウォッチ・名前・値、右にキーフレーム。
    #[allow(clippy::too_many_arguments)]
    fn property_row(&mut self, ui: &mut egui::Ui, comp: &Composition, layer: &Layer, row: &Row, depth: usize, view: TlView, geo: Geo) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
        if !ui.is_rect_visible(rect) {
            return;
        }
        let fr = comp.frame_rate;
        let id = layer.id.clone();
        let rid = ui.id().with(("prop", &id.0, &row.path));
        let now = fr.frame_to_time(self.frame);
        let label = path_label(layer, &row.path);
        self.row_background(ui, rect, geo, false);
        let mut edit: Option<(Time, PropEdit)> = None;

        // ---------- 左 ----------
        let dim = theme::TEXT_DIM;
        if row.animated {
            // 前のキーへ／この時刻にキーを追加・削除／次のキーへ
            let av = |i: usize| geo.cell(geo.x0 + 2.0 + i as f32 * AV_W, AV_W, rect);
            let prev = row.keys.iter().map(|k| k.0).filter(|t| *t < now).max();
            let next = row.keys.iter().map(|k| k.0).filter(|t| *t > now).min();
            if widgets::icon_cell(ui, av(0), rid.with("prev"), Icon::KeyPrev, prev.is_some(), theme::TEXT_OFF, "前のキーフレームへ")
                .clicked()
            {
                if let Some(t) = prev {
                    self.set_frame(fr.time_to_frame(t), false);
                }
            }
            let on = row.on_key.is_some();
            if widgets::icon_cell(
                ui,
                av(1),
                rid.with("key"),
                if on { Icon::DiamondFill } else { Icon::Diamond },
                on,
                dim,
                "この時刻のキーフレームを追加／削除",
            )
            .clicked()
            {
                edit = Some((now, if on { PropEdit::RemoveKey } else { PropEdit::AddKey }));
            }
            if widgets::icon_cell(ui, av(2), rid.with("next"), Icon::KeyNext, next.is_some(), theme::TEXT_OFF, "次のキーフレームへ")
                .clicked()
            {
                if let Some(t) = next {
                    self.set_frame(fr.time_to_frame(t), false);
                }
            }
        }
        let x = geo.x0 + TREE_X + depth as f32 * 12.0;
        let watch = geo.cell(x - 2.0, 16.0, rect);
        let resp = ui.interact(watch, rid.with("watch"), Sense::click()).on_hover_text("ストップウォッチ: アニメーションのオン／オフ");
        icons::paint_sized(
            ui.painter(),
            watch,
            Icon::Stopwatch,
            if row.animated {
                theme::VALUE
            } else if resp.hovered() {
                Color32::WHITE
            } else {
                dim
            },
            13.0,
        );
        if resp.clicked() {
            edit = Some((now, PropEdit::ToggleAnimation));
        }
        // 値（右寄せ）。残りの幅に名前を書く
        let values = Rect::from_min_max(pos2(x + 60.0, rect.top() + 1.0), pos2(geo.split - 6.0, rect.bottom() - 1.0));
        let mut vui = child(ui, values, Layout::right_to_left(Align::Center));
        vui.spacing_mut().interact_size.y = 16.0;
        if let Some(e) = value_widgets(&mut vui, row, true) {
            edit = Some((now, e));
        }
        let label_end = vui.min_rect().left() - 6.0;
        let label_clip = Rect::from_min_max(pos2(x + 16.0, rect.top()), pos2(label_end.max(x + 16.0), rect.bottom()));
        let has_expr = row.expr.is_some();
        ui.painter().with_clip_rect(label_clip).text(
            pos2(label_clip.left(), rect.center().y),
            Align2::LEFT_CENTER,
            if has_expr { format!("{label}  =") } else { label.clone() },
            FontId::proportional(11.0),
            theme::TEXT,
        );

        // ---------- 右: キーフレーム ----------
        let time = Rect::from_min_max(pos2(view.left, rect.top()), pos2(view.left + view.width, rect.bottom()));
        let resp = ui.interact(time, rid.with("keys"), Sense::click_and_drag());
        self.anchors.insert(format!("row:prop:{}:{}", id.0, row.path), time);
        // 先頭フレームのキーフレームが欠けないよう、左の余白まで描く
        let p = ui.painter_at(time.expand2(vec2(6.0, 0.0)));
        let x_of = |t: Time| view.x(t.seconds() * fr.fps());
        let cy = rect.center().y;
        if let (Some(a), Some(b)) = (row.keys.first(), row.keys.last()) {
            // 最初と最後のキーの間を線で結ぶ（アニメーションしている区間）
            p.hline(x_of(a.0)..=x_of(b.0), cy, Stroke::new(1.0_f32, Color32::from_gray(0x48)));
        }
        for (t, kind) in &row.keys {
            key_shape(&p, pos2(x_of(*t), cy), *kind, self.is_key_selected(&id, Some(&row.path), *t));
        }
        let key_at = |px: f32| row.keys.iter().map(|k| k.0).any(|t| (x_of(t) - px).abs() <= GRAB);
        if resp.hover_pos().is_some_and(|h| key_at(h.x)) && self.tl_drag.is_none() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let times: Vec<Time> = row.keys.iter().map(|k| k.0).collect();
        if resp.clicked() || resp.secondary_clicked() || resp.drag_started() {
            // 貼り付け先のプロパティ
            self.key_target = Some((id.clone(), row.path.clone()));
        }
        self.key_lane(ui, comp, layer, &resp, time, &times, Some(&row.path), view, true);
        let has_sel = self.selected_keys.iter().any(|k| k.layer == id && k.path == row.path);
        resp.context_menu(|ui| {
            if has_sel {
                self.key_menu(ui);
                ui.separator();
            }
            if ui.button("現在の時刻にキーフレームを追加").clicked() {
                edit = Some((now, PropEdit::AddKey));
                ui.close_menu();
            }
            if !row.is_path && ui.button("グラフエディターで開く").clicked() {
                self.graph_path = Some(row.path.clone());
                self.show.graph = true;
                ui.close_menu();
            }
        });

        if let Some((at, e)) = edit {
            if matches!(e, PropEdit::RemoveKey) {
                self.selected_keys.retain(|k| !(k.layer == id && k.path == row.path && k.at == at));
            }
            let path = row.path.clone();
            self.edit_layer(&label, e.coalesce(), &id, |l| apply_edit(l, &path, at, &e));
        }
    }
}

/// スペクトログラムを画像にする。横は時間、縦は周波数（対数、下が低音）。
fn spectrogram_image(stereo: &[f32]) -> egui::ColorImage {
    const N: usize = 2048;
    const HEIGHT: usize = 96;
    const MAX_COLS: usize = 2048;
    let mono: Vec<f32> = stereo.chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5).collect();
    // 列数が増えすぎないよう、長い音声ほど粗く刻む
    let hop = (mono.len() / MAX_COLS).max(N / 2);
    let sp = spectrogram(&mono, N, hop);
    let w = sp.columns.len().max(1);
    let mut img = egui::ColorImage::new([w, HEIGHT], Color32::BLACK);
    let (f_lo, f_hi) = (40.0f64, 16_000.0f64);
    for (x, col) in sp.columns.iter().enumerate() {
        for y in 0..HEIGHT {
            let u = 1.0 - y as f64 / (HEIGHT - 1) as f64;
            let freq = f_lo * (f_hi / f_lo).powf(u);
            let bin = ((freq / (SAMPLE_RATE as f64 / N as f64)) as usize).min(col.len() - 1);
            // -80dB..0dB を 0..1 に
            let v = ((col[bin] + 80.0) / 80.0).clamp(0.0, 1.0);
            let c = (v * 255.0) as u8;
            img.pixels[y * w + x] = Color32::from_rgb(c, (v * v * 255.0) as u8, 255 - c / 2).gamma_multiply(0.2 + 0.8 * v);
        }
    }
    img
}
