//! 選択中のキーフレームへの操作: 選択の切り替え、ドラッグでの移動・伸縮、コピー＆ペースト、
//! 補間・イージングプリセットの一括適用、時間反転、右クリックメニュー。
//!
//! 中身の計算は [`znd_core::keys`] にあり、ここは選択（[`KeySel`]）とレイヤーの対応付けと取り消しの単位を受け持つ。

use crate::keymap::Action;
use crate::panels::KeyKind;
use crate::timeline::{KeySel, TlDrag, TlView};
use crate::{theme, App};
use eframe::egui;
use znd_core::keys::{self, KeyAddr, KeyClipboard, KeyStyle, EASE_PRESETS};
use znd_core::*;

impl KeyKind {
    /// 一括変更用の指定にする。
    pub(crate) fn style(self) -> KeyStyle {
        let both = |i, o, m| KeyStyle { interp_in: Some(i), interp_out: Some(o), bezier_mode: Some(m), ease_in: None, ease_out: None };
        match self {
            KeyKind::Linear => both(Interp::Linear, Interp::Linear, BezierMode::Manual),
            KeyKind::EasyEase => both(Interp::Bezier, Interp::Bezier, BezierMode::Manual),
            KeyKind::EaseIn => both(Interp::Bezier, Interp::Linear, BezierMode::Manual),
            KeyKind::EaseOut => both(Interp::Linear, Interp::Bezier, BezierMode::Manual),
            KeyKind::AutoBezier => both(Interp::Bezier, Interp::Bezier, BezierMode::Auto),
            KeyKind::Hold => KeyStyle { interp_out: Some(Interp::Hold), bezier_mode: Some(BezierMode::Manual), ..KeyStyle::new() },
        }
    }
}

/// 選択をレイヤーごとにまとめる。
fn by_layer(sel: &[KeySel]) -> Vec<(LayerId, Vec<KeyAddr>)> {
    let mut out: Vec<(LayerId, Vec<KeyAddr>)> = Vec::new();
    for k in sel {
        let addr = KeyAddr::new(&k.path, k.at);
        match out.iter_mut().find(|(l, _)| *l == k.layer) {
            Some((_, v)) => v.push(addr),
            None => out.push((k.layer.clone(), vec![addr])),
        }
    }
    out
}

fn to_sel(layer: &LayerId, addrs: Vec<KeyAddr>) -> impl Iterator<Item = KeySel> + '_ {
    addrs.into_iter().map(move |a| KeySel { layer: layer.clone(), path: a.path, at: a.time })
}

/// レイヤーの、時刻 `at` にあるキー。`prefix` を指定すると、そのプロパティ（の配下）だけ。
pub(crate) fn expand(layer: &Layer, prefix: Option<&str>, at: Time) -> Vec<KeySel> {
    keys::keys_at(layer, at)
        .into_iter()
        .filter(|k| prefix.is_none_or(|p| keys::path_matches(&k.path, p)))
        .map(|k| KeySel { layer: layer.id.clone(), path: k.path, at: k.time })
        .collect()
}

impl App {
    pub(crate) fn is_key_selected(&self, layer: &LayerId, prefix: Option<&str>, at: Time) -> bool {
        self.selected_keys.iter().any(|k| &k.layer == layer && k.at == at && prefix.is_none_or(|p| keys::path_matches(&k.path, p)))
    }

    /// キーをクリックしたときの選択。Shift / ⌘ を押していれば足し引きし、押していなければ置き換える。
    pub(crate) fn click_keys(&mut self, picked: Vec<KeySel>, additive: bool) {
        if !additive {
            self.selected_keys = picked;
            return;
        }
        let all_in = !picked.is_empty() && picked.iter().all(|k| self.selected_keys.contains(k));
        if all_in {
            self.selected_keys.retain(|k| !picked.contains(k));
        } else {
            for k in picked {
                if !self.selected_keys.contains(&k) {
                    self.selected_keys.push(k);
                }
            }
        }
    }

    /// キーを掴んだ。掴んだキーが選択に入っていなければ、それだけを選ぶ。
    /// Alt を押しながら選択の先頭か末尾のキーを掴むと、反対の端を中心にした伸縮になる。
    pub(crate) fn begin_key_drag(&mut self, comp: &Composition, picked: Vec<KeySel>, grab: Time, alt: bool) {
        if picked.is_empty() {
            return;
        }
        if !picked.iter().all(|k| self.selected_keys.contains(k)) {
            self.selected_keys = picked;
        }
        let pivot = alt
            .then(|| {
                let lo = self.selected_keys.iter().map(|k| k.at).min()?;
                let hi = self.selected_keys.iter().map(|k| k.at).max()?;
                if lo == hi {
                    None
                } else if grab == hi {
                    Some(lo)
                } else if grab == lo {
                    Some(hi)
                } else {
                    None
                }
            })
            .flatten();
        let base: Vec<Layer> =
            by_layer(&self.selected_keys).iter().filter_map(|(id, _)| comp.layer(id)).filter(|l| !l.locked).cloned().collect();
        self.tl_drag = Some(TlDrag::Keys { grab, last: grab, pivot, orig: self.selected_keys.clone(), base });
        self.last_edit = None;
    }

    /// 掴んだキーをポインタの位置 `x` へ。ドラッグを始めたときの状態から毎回作り直すので、
    /// 途中で他のキーを通り過ぎても消えない（離した位置で重なったものだけが置き換わる）。
    pub(crate) fn drag_keys(&mut self, comp: &Composition, view: TlView, x: f32, snap: bool) {
        let Some(TlDrag::Keys { grab, last, pivot, orig, base }) = &self.tl_drag else { return };
        let exclude = orig.first().map(|k| k.layer.clone()).unwrap_or_else(|| LayerId::new(""));
        let to = self.pointer_time(comp, view, x, &exclude, snap);
        if to == *last {
            return;
        }
        let (grab, pivot, orig, base) = (*grab, *pivot, orig.clone(), base.clone());
        let fr = comp.frame_rate;
        // 伸縮で中心を越えたら反転になる。中心ちょうどは潰れてしまうので止める
        if pivot == Some(to) {
            return;
        }
        let mut new_sel = Vec::new();
        let groups = by_layer(&orig);
        let label = if pivot.is_some() { "キーフレームを伸縮" } else { "キーフレームを移動" };
        self.edit_comp(label, true, |c| {
            for b in &base {
                let Some(l) = c.layer_mut(&b.id) else { continue };
                *l = b.clone();
                let Some((_, addrs)) = groups.iter().find(|(id, _)| *id == b.id) else { continue };
                let out = match pivot {
                    Some(p) => keys::scale(l, addrs, p, (to - p).0 as f64 / (grab - p).0 as f64, fr),
                    None => keys::shift(l, addrs, to - grab),
                };
                new_sel.extend(to_sel(&b.id, out));
            }
        });
        if let Some(TlDrag::Keys { last, .. }) = &mut self.tl_drag {
            *last = to;
        }
        if !new_sel.is_empty() {
            self.selected_keys = new_sel;
        }
    }

    /// 選択中のキーに `f` を適用する（レイヤーごと）。`f` が新しい選択を返せば置き換える。
    pub(crate) fn edit_selected_keys(&mut self, label: &str, f: impl Fn(&mut Layer, &[KeyAddr]) -> Option<Vec<KeyAddr>>) {
        let sel = self.selected_keys.clone();
        self.edit_keys(label, &sel, f);
    }

    /// `sel` のキーに `f` を適用する（レイヤーごと）。`f` が新しい選択を返せば、選択をそれで置き換える。
    pub(crate) fn edit_keys(&mut self, label: &str, sel: &[KeySel], f: impl Fn(&mut Layer, &[KeyAddr]) -> Option<Vec<KeyAddr>>) {
        let groups = by_layer(sel);
        if groups.is_empty() {
            return;
        }
        let mut new_sel: Option<Vec<KeySel>> = None;
        self.edit_comp(label, false, |c| {
            for (id, addrs) in &groups {
                let Some(l) = c.layer_mut(id).filter(|l| !l.locked) else { continue };
                if let Some(out) = f(l, addrs) {
                    new_sel.get_or_insert_with(Vec::new).extend(to_sel(id, out));
                }
            }
        });
        if let Some(s) = new_sel {
            self.selected_keys = s;
        }
    }

    pub(crate) fn delete_selected_keys(&mut self) {
        let n = self.selected_keys.len();
        self.edit_selected_keys("キーフレーム削除", |l, a| {
            keys::delete(l, a);
            Some(Vec::new())
        });
        self.selected_keys.clear();
        self.status = format!("キーフレームを {n} 個削除しました");
    }

    pub(crate) fn style_selected_keys(&mut self, label: &str, style: KeyStyle) {
        let sel = self.selected_keys.clone();
        self.style_keys(label, &sel, style);
    }

    pub(crate) fn style_keys(&mut self, label: &str, sel: &[KeySel], style: KeyStyle) {
        self.edit_keys(label, sel, |l, a| {
            keys::set_style(l, a, &style);
            None
        });
    }

    pub(crate) fn reverse_selected_keys(&mut self) {
        // レイヤーをまたぐときも、選択全体の最初と最後を保って反転する
        let (Some(lo), Some(hi)) = (self.selected_keys.iter().map(|k| k.at).min(), self.selected_keys.iter().map(|k| k.at).max()) else {
            return;
        };
        self.edit_selected_keys("キーフレームの時間反転", |l, a| Some(keys::reverse_between(l, a, lo, hi)));
    }

    /// 選択キーの時間を `percent` % に伸縮する（先頭のキーを中心に）。
    pub(crate) fn scale_selected_keys(&mut self, percent: f64) {
        let Some(lo) = self.selected_keys.iter().map(|k| k.at).min() else { return };
        let Some(fr) = self.shared.comp(&self.comp_id).map(|c| c.frame_rate) else { return };
        let f = percent / 100.0;
        if f.abs() < 1e-6 {
            return;
        }
        self.edit_selected_keys("キーフレームを伸縮", |l, a| Some(keys::scale(l, a, lo, f, fr)));
    }

    pub(crate) fn copy_selected_keys(&mut self, ctx: &egui::Context) -> bool {
        let shared = self.shared.clone();
        let Some(comp) = shared.comp(&self.comp_id) else { return false };
        let groups = by_layer(&self.selected_keys);
        if groups.is_empty() {
            return false;
        }
        // 複数のレイヤーから取ったときは、先頭のレイヤーのぶんだけ（貼り付け先は 1 つのレイヤー）
        let (id, addrs) = &groups[0];
        let Some(layer) = comp.layer(id) else { return false };
        let clip = keys::copy(layer, addrs);
        if clip.is_empty() {
            return false;
        }
        ctx.copy_text(clip.to_text());
        self.status = format!("キーフレームを {} 個コピーしました", clip.key_count());
        self.key_clip = Some(clip);
        true
    }

    /// 選択中のレイヤーの、再生ヘッドの位置に貼り付ける。
    /// プロパティを 1 つだけコピーしていて、最後にクリックしたプロパティの行がそのレイヤーにあれば、そこへ貼る。
    pub(crate) fn paste_keys(&mut self, clip: Option<KeyClipboard>) -> bool {
        let Some(clip) = clip.or_else(|| self.key_clip.clone()) else { return false };
        let Some(id) = self.selected.clone() else {
            self.status = "キーフレームを貼り付けるレイヤーを選択してください".into();
            return false;
        };
        let Some(comp) = self.shared.comp(&self.comp_id) else { return false };
        if comp.layer(&id).is_some_and(|l| l.locked) {
            self.status = "ロック中のレイヤーには貼り付けられません".into();
            return false;
        }
        let at = comp.frame_rate.frame_to_time(self.frame);
        let target = self.key_target.clone().filter(|(l, _)| *l == id).map(|(_, p)| p);
        let mut pasted = Vec::new();
        self.edit_layer("キーフレームを貼り付け", false, &id, |l| pasted = keys::paste(l, &clip, at, target.as_deref()));
        if pasted.is_empty() {
            self.status = "貼り付けられるプロパティがありません（型が違います）".into();
            return false;
        }
        self.status = format!("キーフレームを {} 個貼り付けました", pasted.len());
        self.selected_keys = to_sel(&id, pasted).collect();
        true
    }

    pub(crate) fn select_all_keys(&mut self) {
        let Some(layer) = self.selected.as_ref().and_then(|id| self.shared.comp(&self.comp_id)?.layer(id).cloned()) else { return };
        self.selected_keys = to_sel(&layer.id, keys::all_keys(&layer)).collect();
    }

    /// ⌘C / ⌘V / ⌘A（文字入力中でないとき）。egui はこれらをキーではなくイベントで渡してくる。
    pub(crate) fn key_clipboard_events(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() {
            return;
        }
        let events = ctx.input(|i| i.events.clone());
        for e in events {
            match e {
                egui::Event::Copy if !self.selected_keys.is_empty() => {
                    self.copy_selected_keys(ctx);
                }
                egui::Event::Paste(text) => {
                    let clip = KeyClipboard::from_text(&text);
                    if clip.is_some() || self.key_clip.is_some() {
                        self.paste_keys(clip);
                    }
                }
                _ => {}
            }
        }
    }

    /// キーの右クリックメニュー（タイムライン・アニメーションメニュー共通）。
    pub(crate) fn key_menu(&mut self, ui: &mut egui::Ui) {
        let n = self.selected_keys.len();
        let dim = theme::TEXT_DIM;
        ui.set_min_width(200.0);
        ui.label(egui::RichText::new(format!("選択中のキーフレーム: {n} 個")).size(10.0).color(dim));
        ui.add_enabled_ui(n > 0, |ui| {
            ui.menu_button("キーフレーム補間", |ui| {
                for k in KeyKind::ALL {
                    let sc = match k {
                        KeyKind::EasyEase => self.shortcut_text(Action::EasyEase),
                        KeyKind::EaseIn => self.shortcut_text(Action::EaseIn),
                        KeyKind::EaseOut => self.shortcut_text(Action::EaseOut),
                        KeyKind::Linear => self.shortcut_text(Action::LinearKeys),
                        KeyKind::Hold => self.shortcut_text(Action::HoldKeys),
                        KeyKind::AutoBezier => String::new(),
                    };
                    if ui.add(egui::Button::new(k.label()).shortcut_text(sc)).clicked() {
                        ui.close_menu();
                        self.style_selected_keys("キーフレーム補間", k.style());
                    }
                }
            });
            ui.menu_button("イージングプリセット", |ui| {
                for p in EASE_PRESETS {
                    if ui.button(p.name).clicked() {
                        ui.close_menu();
                        self.style_selected_keys("イージング", p.style);
                    }
                }
            });
            ui.separator();
            if ui.add(egui::Button::new("時間反転").shortcut_text(self.shortcut_text(Action::ReverseKeys))).clicked() {
                ui.close_menu();
                self.reverse_selected_keys();
            }
            ui.horizontal(|ui| {
                let id = ui.id().with("scale-pct");
                let mut pct: f64 = ui.data_mut(|d| *d.get_temp_mut_or(id, 200.0));
                ui.label("時間伸縮");
                ui.add(egui::DragValue::new(&mut pct).range(-1000.0..=1000.0).suffix(" %").speed(1.0));
                ui.data_mut(|d| d.insert_temp(id, pct));
                if ui.button("適用").clicked() {
                    ui.close_menu();
                    self.scale_selected_keys(pct);
                }
            });
            ui.separator();
            let c = ui.ctx().format_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::C));
            if ui.add(egui::Button::new("コピー").shortcut_text(c)).clicked() {
                ui.close_menu();
                let ctx = ui.ctx().clone();
                self.copy_selected_keys(&ctx);
            }
        });
        let v = ui.ctx().format_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::V));
        if ui.add_enabled(self.key_clip.is_some(), egui::Button::new("再生ヘッドの位置に貼り付け").shortcut_text(v)).clicked() {
            ui.close_menu();
            self.paste_keys(None);
        }
        if ui.add_enabled(n > 0, egui::Button::new("削除").shortcut_text(self.shortcut_text(Action::Delete))).clicked() {
            ui.close_menu();
            self.delete_selected_keys();
        }
    }
}
