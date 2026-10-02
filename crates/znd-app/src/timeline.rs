//! タイムラインパネル: ルーラー（スクラブ・ビートグリッド・マーカー・警告）、音声の波形／スペクトログラム、レイヤー行。
//!
//! 横方向は拡大・スクロールできる（⌘ + ホイールで拡大、ホイールで横スクロール）。最大まで拡大すると
//! 波形が 1 サンプルずつ見える。レイヤーのバーは掴んで移動・端を掴んでトリム、キーフレームは掴んで移動でき、
//! いずれもビートグリッド・マーカー・再生ヘッド・他のレイヤーの端に吸着する（Shift で吸着なし）。

use crate::App;
use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, FontId, Layout, Rect, Sense, Shape, Stroke};
use znd_audio::analysis::spectrogram;
use znd_audio::SAMPLE_RATE;
use znd_core::*;

/// レイヤー名・スイッチ列の幅。
const LEFT: f32 = 300.0;
const ROW: f32 = 22.0;
const AUDIO_ROW: f32 = 56.0;
const NAME_CHARS: usize = 14;

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
    /// 時刻 `at` にあるキーフレーム（全プロパティ）の移動。
    Keys {
        layer: LayerId,
        at: Time,
    },
}

fn frame_x(view: TlView, frame: f64) -> f32 {
    view.x(frame)
}

fn playhead(p: &egui::Painter, rect: Rect, frame: i64, view: TlView) {
    let x = view.x(frame as f64);
    p.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.5_f32, Color32::from_rgb(255, 90, 90)));
}

fn kind_style(kind: &LayerKind) -> (&'static str, Color32) {
    match kind {
        LayerKind::Solid { .. } => ("■", Color32::from_rgb(170, 80, 80)),
        LayerKind::Footage { .. } => ("🎞", Color32::from_rgb(80, 130, 170)),
        LayerKind::Precomp { .. } => ("▣", Color32::from_rgb(150, 130, 80)),
        LayerKind::Null => ("□", Color32::from_rgb(120, 120, 120)),
        LayerKind::Adjustment => ("◐", Color32::from_rgb(130, 100, 160)),
        LayerKind::Shape { .. } => ("★", Color32::from_rgb(90, 150, 200)),
        LayerKind::Text { .. } => ("T", Color32::from_rgb(190, 80, 110)),
        LayerKind::Audio { .. } => ("♪", Color32::from_rgb(70, 160, 120)),
        LayerKind::Camera { .. } => ("📷", Color32::from_rgb(190, 150, 110)),
        LayerKind::Light { .. } => ("💡", Color32::from_rgb(200, 190, 100)),
        LayerKind::Model { .. } => ("◆", Color32::from_rgb(110, 110, 190)),
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

impl App {
    pub(crate) fn timeline(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let shared = self.shared.clone();
        let Some(comp) = shared.comp(&self.comp_id) else { return };
        let total = comp.frame_count().max(1);
        let fr = comp.frame_rate;
        // (倍率, 中心にする画面 x)。ボタンやホイールからの拡大・縮小の要求
        let mut zoom_request: Option<(f64, Option<f32>)> = None;
        let mut view = self.tl_view.unwrap_or(TlView { left: 0.0, width: 1.0, start: 0.0, span: total as f64 });

        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(vec2(LEFT, ROW), Layout::left_to_right(Align::Center), |ui| {
                ui.set_width(LEFT);
                ui.checkbox(&mut self.hide_shy, "シャイを隠す");
                ui.weak(format!("{} レイヤー", comp.layers.len()));
                ui.separator();
                if ui.small_button("−").on_hover_text("縮小（⌘ + ホイール）").clicked() {
                    zoom_request = Some((0.5, None));
                }
                if ui.small_button("＋").on_hover_text("拡大（⌘ + ホイール）").clicked() {
                    zoom_request = Some((2.0, None));
                }
                if ui.small_button("全体").on_hover_text("コンポ全体を表示").clicked() {
                    self.tl_zoom = 1.0;
                    self.tl_scroll = 0.0;
                }
                if self.tl_zoom > 1.0 {
                    ui.weak(format!("×{:.0}", self.tl_zoom));
                }
            });
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click_and_drag());
            // 表示範囲を決める（拡大率の上限は 1 フレームが MAX_PX_PER_FRAME になるまで）
            let max_zoom = (total as f64 * MAX_PX_PER_FRAME / rect.width().max(1.0) as f64).max(1.0);
            self.tl_zoom = self.tl_zoom.clamp(1.0, max_zoom);
            let span = total as f64 / self.tl_zoom;
            self.tl_scroll = self.tl_scroll.clamp(0.0, total as f64 - span);
            view = TlView { left: rect.left(), width: rect.width(), start: self.tl_scroll, span };
            self.tl_view = Some(view);
            self.anchors.insert("ruler".into(), rect);
            let p = ui.painter_at(rect);
            p.rect_filled(rect, 0.0, Color32::from_gray(40));

            // ワークエリア
            if let Some((a, b)) = self.work_area {
                let r = Rect::from_min_max(pos2(frame_x(view, a as f64), rect.top()), pos2(frame_x(view, b as f64), rect.top() + 4.0));
                p.rect_filled(r, 0.0, Color32::from_rgb(120, 160, 230));
            }
            // RAM プレビュー済みのフレーム
            for f in self.engine.cached_frames(total, self.downsample) {
                let x0 = frame_x(view, f as f64);
                let x1 = frame_x(view, f as f64 + 1.0).max(x0 + 1.0);
                p.rect_filled(
                    Rect::from_min_max(pos2(x0, rect.bottom() - 3.0), pos2(x1, rect.bottom())),
                    0.0,
                    Color32::from_rgb(60, 180, 90),
                );
            }
            // 光過敏性の警告区間
            for w in &self.check.flashes {
                let r = Rect::from_min_max(
                    pos2(frame_x(view, w.start_frame as f64), rect.bottom() - 7.0),
                    pos2(frame_x(view, w.end_frame as f64 + 1.0), rect.bottom() - 3.0),
                );
                p.rect_filled(r, 0.0, Color32::from_rgb(255, 60, 60));
            }

            // 秒目盛り（ラベルが重ならない間隔を選ぶ）。十分拡大したらフレーム単位の目盛りにする
            let px_per_sec = view.px_per_frame() * fr.fps();
            let x_of = |t: Time| view.x(t.seconds() * fr.fps());
            let (first, last) = (view.start.floor() as i64, (view.start + view.span).ceil() as i64);
            if view.px_per_frame() >= 8.0 {
                let label_every = [1i64, 2, 5, 10, 15, 30].into_iter().find(|n| *n as f64 * view.px_per_frame() >= 50.0).unwrap_or(30);
                for f in first.max(0)..=last.min(total) {
                    let x = view.x(f as f64);
                    p.line_segment([pos2(x, rect.top()), pos2(x, rect.top() + 4.0)], Stroke::new(1.0_f32, Color32::from_gray(110)));
                    if f % label_every == 0 {
                        p.text(pos2(x + 3.0, rect.top() + 1.0), Align2::LEFT_TOP, format!("{f}f"), FontId::monospace(10.0), Color32::GRAY);
                    }
                }
            } else {
                let step = [1u32, 2, 5, 10, 30, 60].into_iter().find(|s| *s as f64 * px_per_sec >= 50.0).unwrap_or(60);
                let seconds = comp.duration.seconds();
                for s in (0..).map(|i| i * step).take_while(|s| (*s as f64) < seconds) {
                    let x = view.x(s as f64 * fr.fps());
                    if x < rect.left() - 60.0 || x > rect.right() {
                        continue;
                    }
                    p.line_segment([pos2(x, rect.top()), pos2(x, rect.top() + 6.0)], Stroke::new(1.0_f32, Color32::GRAY));
                    p.text(pos2(x + 3.0, rect.top() + 1.0), Align2::LEFT_TOP, format!("{s}s"), FontId::monospace(10.0), Color32::GRAY);
                }
            }
            // ビートグリッド（線が密になりすぎるときは拍頭・小節頭だけにする）
            if let Some(g) = &comp.beat_grid {
                // 見えている範囲の格子だけを描く
                let (from, to) = (fr.frame_to_time(first.max(0)), fr.frame_to_time(last.min(total) + 1));
                let lines = g.lines(from, to);
                let dense = lines.len() as f32 > rect.width() / 4.0;
                for (t, bar, beat) in lines {
                    if dense && !beat {
                        continue;
                    }
                    let x = x_of(t);
                    let (len, col) = if bar {
                        (ROW, Color32::from_rgb(230, 200, 120))
                    } else if beat {
                        (10.0, Color32::from_rgb(150, 135, 90))
                    } else {
                        (5.0, Color32::from_gray(90))
                    };
                    p.line_segment([pos2(x, rect.bottom() - len), pos2(x, rect.bottom())], Stroke::new(1.0_f32, col));
                }
            }

            for m in &comp.markers {
                let x = x_of(m.time);
                let tip = pos2(x, rect.bottom() - 3.0);
                p.add(Shape::convex_polygon(
                    vec![tip, pos2(x - 4.0, tip.y - 7.0), pos2(x + 4.0, tip.y - 7.0)],
                    Color32::from_rgb(240, 200, 60),
                    Stroke::NONE,
                ));
            }
            // レビューコメント
            for r in shared.reviews.iter().filter(|r| r.comp == comp.id && !r.resolved) {
                p.circle_filled(pos2(x_of(r.time), rect.top() + 6.0), 3.5, Color32::from_rgb(80, 180, 255));
            }
            playhead(&p, rect, self.frame, view);

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
                            .find(|w| hover.x >= frame_x(view, w.start_frame as f64) && hover.x <= frame_x(view, w.end_frame as f64 + 1.0))
                            .map(|w| w.message())
                    });
                if let Some(text) = tip {
                    resp.clone().on_hover_text_at_pointer(text);
                }
            }
            if resp.dragged() || resp.clicked() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    // Shift を押している間はスナップしない
                    let free = ui.input(|i| i.modifiers.shift);
                    self.set_frame(view.frame(pos.x).floor() as i64, !free);
                }
            }
        });

        // 音声: 波形またはスペクトログラム
        if self.has_audio() {
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(vec2(LEFT, AUDIO_ROW), Layout::left_to_right(Align::Center), |ui| {
                    ui.set_width(LEFT);
                    ui.label("♪ 音声");
                    ui.selectable_value(&mut self.show_spectrogram, false, "波形");
                    ui.selectable_value(&mut self.show_spectrogram, true, "スペクトログラム");
                });
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), AUDIO_ROW), Sense::click_and_drag());
                let p = ui.painter_at(rect);
                p.rect_filled(rect, 0.0, Color32::from_gray(26));
                let buf = self.audio_buf.clone();
                let frames = buf.len() / 2;
                self.anchors.insert("wave".into(), rect);
                // 表示範囲に対応するサンプル位置（小数）
                let samples_per_frame = SAMPLE_RATE as f64 / fr.fps();
                let (s0, s1) = (view.start * samples_per_frame, (view.start + view.span) * samples_per_frame);
                let wave_color = Color32::from_rgb(90, 200, 150);
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
                        p.line_segment([pos2(rect.left(), mid), pos2(rect.right(), mid)], Stroke::new(1.0_f32, Color32::from_gray(50)));
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
                            let col = if bar {
                                Color32::from_rgba_unmultiplied(230, 200, 120, 110)
                            } else {
                                Color32::from_rgba_unmultiplied(230, 200, 120, 40)
                            };
                            p.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.0_f32, col));
                        }
                    }
                }
                playhead(&p, rect, self.frame, view);
                if resp.dragged() || resp.clicked() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let free = ui.input(|i| i.modifiers.shift);
                        self.set_frame(view.frame(pos.x).floor() as i64, !free);
                    }
                }
            });
        } else if let Some(task) = &self.audio_task {
            ui.weak(format!("♪ {}…", task.label));
        }
        ui.separator();

        // 表示されていない行の位置は覚えておかない
        self.anchors.retain(|k, _| !k.starts_with("row:"));
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for layer in &comp.layers {
                if !(self.hide_shy && layer.shy) {
                    self.layer_row(ui, comp, layer, view);
                }
            }
        });

        // 拡大・縮小とスクロール（タイムラインの上にポインタがあるとき）
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let over = pointer.is_some_and(|p| p.x >= view.left && p.x <= view.left + view.width && ui.clip_rect().contains(p));
        if over {
            let (scroll, zoom_delta, cmd) = ui.input(|i| (i.raw_scroll_delta, i.zoom_delta(), i.modifiers.command));
            if (zoom_delta - 1.0).abs() > 1e-3 {
                // ⌘ + ホイール、またはピンチ
                zoom_request = Some((zoom_delta as f64, pointer.map(|p| p.x)));
            } else if !cmd && scroll.x.abs() > 0.0 {
                self.tl_scroll -= scroll.x as f64 / view.px_per_frame();
            }
        }
        if let Some((factor, at)) = zoom_request {
            // ポインタの下（ボタンからなら再生ヘッド）のフレームが動かないように拡大する
            let pivot = at.map(|x| view.frame(x)).unwrap_or(self.frame as f64);
            let ratio = at.map(|x| ((x - view.left) / view.width) as f64).unwrap_or(0.5);
            self.tl_zoom = (self.tl_zoom * factor).max(1.0);
            let max_zoom = (total as f64 * MAX_PX_PER_FRAME / view.width.max(1.0) as f64).max(1.0);
            self.tl_zoom = self.tl_zoom.min(max_zoom);
            let span = total as f64 / self.tl_zoom;
            self.tl_scroll = (pivot - ratio * span).clamp(0.0, (total as f64 - span).max(0.0));
        }
        if ui.input(|i| i.pointer.any_released()) && self.tl_drag.is_some() {
            self.tl_drag = None;
            // ドラッグごとに取り消しの単位を分ける
            self.last_edit = None;
        }
    }

    /// ポインタの位置 `x` を時刻にする。`snap` なら、ビートグリッド・マーカー・再生ヘッド・他のレイヤーの端に吸着する。
    fn pointer_time(&self, comp: &Composition, view: TlView, x: f32, exclude: &LayerId, snap: bool) -> Time {
        let fr = comp.frame_rate;
        let t = Time::from_seconds(view.frame(x) / fr.fps());
        if !snap {
            return beat::nearest_frame(t, fr);
        }
        // 画面上で 6px 以内なら吸着
        let tolerance = Time::from_seconds(6.0 / view.px_per_frame() / fr.fps());
        edit::snap_time(comp, t, Some(exclude), &[fr.frame_to_time(self.frame)], tolerance)
    }

    fn layer_row(&mut self, ui: &mut egui::Ui, comp: &Composition, layer: &Layer, view: TlView) {
        let fr = comp.frame_rate;
        let id = layer.id.clone();
        let selected = self.selected.as_ref() == Some(&id);
        let (icon, color) = kind_style(&layer.kind);

        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(vec2(LEFT, ROW), Layout::left_to_right(Align::Center), |ui| {
                ui.set_width(LEFT);
                if ui.selectable_label(layer.visible, "👁").on_hover_text("表示").clicked() {
                    let v = !layer.visible;
                    self.edit_layer("表示", false, &id, |l| l.visible = v);
                }
                if ui.selectable_label(layer.solo, "S").on_hover_text("ソロ").clicked() {
                    let v = !layer.solo;
                    self.edit_layer("ソロ", false, &id, |l| l.solo = v);
                }
                if ui.selectable_label(layer.locked, "🔒").on_hover_text("ロック").clicked() {
                    let v = !layer.locked;
                    self.edit_layer("ロック", false, &id, |l| l.locked = v);
                }
                let mut name: String = layer.name.chars().take(NAME_CHARS).collect();
                if layer.name.chars().count() > NAME_CHARS {
                    name.push('…');
                }
                if ui.selectable_label(selected, format!("{icon} {name}")).clicked() {
                    self.selected = Some(id.clone());
                    self.selected_footage = None;
                }
                let mut tags = String::new();
                if !layer.effects.is_empty() {
                    tags.push_str("fx ");
                }
                if layer.three_d {
                    tags.push_str("3D ");
                }
                if layer.motion_blur {
                    tags.push_str("MB ");
                }
                if layer.blend_mode != BlendMode::Normal {
                    tags.push_str(layer.blend_mode.label_ja());
                }
                if !tags.is_empty() {
                    ui.weak(tags);
                }
            });

            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click_and_drag());
            self.anchors.insert(format!("row:{}", id.0), rect);
            let p = ui.painter_at(rect);
            p.rect_filled(rect, 0.0, Color32::from_gray(if selected { 52 } else { 34 }));
            let x_of = |t: Time| view.x(t.seconds() * fr.fps());
            let (x0, x1) = (x_of(layer.in_point), x_of(layer.out_point));
            let bar = Rect::from_min_max(pos2(x0, rect.top() + 3.0), pos2(x1, rect.bottom() - 3.0));
            let fill = if layer.visible { color } else { color.gamma_multiply(0.35) };
            p.rect_filled(bar, 3.0, fill);
            let keys = key_times(layer);
            for t in &keys {
                let c = pos2(x_of(*t), rect.center().y);
                let r = 4.0;
                let picked = self.selected_key.as_ref().is_some_and(|(l, at)| l == &id && at == t);
                p.add(Shape::convex_polygon(
                    vec![pos2(c.x, c.y - r), pos2(c.x + r, c.y), pos2(c.x, c.y + r), pos2(c.x - r, c.y)],
                    if picked { Color32::from_rgb(255, 210, 60) } else { Color32::WHITE },
                    Stroke::new(1.0_f32, Color32::BLACK),
                ));
            }
            for m in &layer.markers {
                let x = x_of(m.time);
                p.line_segment(
                    [pos2(x, rect.top() + 2.0), pos2(x, rect.bottom() - 2.0)],
                    Stroke::new(1.5_f32, Color32::from_rgb(240, 200, 60)),
                );
            }
            playhead(&p, rect, self.frame, view);

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
            if resp.drag_started() && !locked {
                if let Some(origin) = ui.input(|i| i.pointer.press_origin()) {
                    let x = origin.x;
                    self.selected = Some(id.clone());
                    self.selected_footage = None;
                    // バーの端にキーフレームが重なっているときは、バーの外側を掴めばトリム、内側ならキーフレーム
                    let outside_edge = (x <= x0 && x0 - x <= GRAB) || (x >= x1 && x - x1 <= GRAB);
                    self.tl_drag = if let Some(at) = key_at(x).filter(|_| !outside_edge) {
                        self.selected_key = Some((id.clone(), at));
                        Some(TlDrag::Keys { layer: id.clone(), at })
                    } else if (x - x0).abs() <= GRAB {
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
                            let (d_in, d_out) =
                                ((snapped_in - round).0.abs(), (snapped_out - beat::nearest_frame(raw_in + len, fr)).0.abs());
                            let target = if d_out != 0 && (d_in == 0 || d_out < d_in) { snapped_out - len } else { snapped_in };
                            let dt = target - layer.in_point;
                            if dt != Time::ZERO {
                                self.edit_layer("レイヤーを移動", true, lid, |l| edit::shift_layer(l, dt));
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
                        Some(TlDrag::Keys { layer: lid, at }) => {
                            let t = self.pointer_time(comp, view, pos.x, lid, snap);
                            if t != *at {
                                let from = *at;
                                self.edit_layer("キーフレームを移動", true, lid, |l| {
                                    edit::move_keys(l, from, t);
                                });
                                *at = t;
                                self.selected_key = Some((lid.clone(), t));
                            }
                        }
                        None => {}
                    }
                    self.tl_drag = drag;
                }
            }
            if resp.clicked() {
                self.selected = Some(id.clone());
                self.selected_footage = None;
                // キーフレームをクリックしたら選択する（Delete で削除、ドラッグで移動）
                self.selected_key = resp.interact_pointer_pos().and_then(|pos| key_at(pos.x)).map(|at| (id.clone(), at));
            }
        });
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
