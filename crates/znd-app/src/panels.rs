//! メニューバーと左パネル（プロジェクト・コンポ設定・素材の権利情報・レイヤーのプロパティ）。

use crate::icons::{self, Icon};
use crate::{base_dir, theme, widgets, App, Task};
use eframe::egui::{self, pos2, vec2, Align2, FontId};
use std::sync::OnceLock;
use znd_audio::analysis::{band_keyframes, detect_onsets, Band};
use znd_audio::Mixer;
use znd_core::effect::{all_effects, effect_def, Pd};
use znd_core::shape::MaskMode;
use znd_core::text::{AnimatorProps, Justify, SelectorUnit, TextStroke};
use znd_core::visit::{visit_layer, visit_layer_mut, PropMut, PropRef};
use znd_core::*;

/// キーフレームの補間種別（UI 上の選択肢）。
#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) enum KeyKind {
    Linear,
    EasyEase,
    EaseIn,
    EaseOut,
    AutoBezier,
    Hold,
}

impl KeyKind {
    pub const ALL: [KeyKind; 6] =
        [KeyKind::Linear, KeyKind::EasyEase, KeyKind::EaseIn, KeyKind::EaseOut, KeyKind::AutoBezier, KeyKind::Hold];

    pub fn label(self) -> &'static str {
        match self {
            KeyKind::Linear => "リニア",
            KeyKind::EasyEase => "イージーイーズ",
            KeyKind::EaseIn => "イーズイン",
            KeyKind::EaseOut => "イーズアウト",
            KeyKind::AutoBezier => "自動ベジェ",
            KeyKind::Hold => "停止",
        }
    }

    pub(crate) fn of<V>(k: &Keyframe<V>) -> KeyKind {
        match (k.interp_in, k.interp_out, k.bezier_mode) {
            (_, Interp::Hold, _) => KeyKind::Hold,
            (Interp::Bezier, Interp::Bezier, BezierMode::Auto) => KeyKind::AutoBezier,
            (Interp::Bezier, Interp::Bezier, _) => KeyKind::EasyEase,
            (Interp::Bezier, _, _) => KeyKind::EaseIn,
            (_, Interp::Bezier, _) => KeyKind::EaseOut,
            _ => KeyKind::Linear,
        }
    }

    fn apply<V>(self, k: &mut Keyframe<V>) {
        let (interp_in, interp_out, mode) = match self {
            KeyKind::Linear => (Interp::Linear, Interp::Linear, BezierMode::Manual),
            KeyKind::EasyEase => (Interp::Bezier, Interp::Bezier, BezierMode::Manual),
            // イーズイン = このキーに入ってくる側だけ減速する
            KeyKind::EaseIn => (Interp::Bezier, Interp::Linear, BezierMode::Manual),
            KeyKind::EaseOut => (Interp::Linear, Interp::Bezier, BezierMode::Manual),
            KeyKind::AutoBezier => (Interp::Bezier, Interp::Bezier, BezierMode::Auto),
            KeyKind::Hold => (k.interp_in, Interp::Hold, BezierMode::Manual),
        };
        k.interp_in = interp_in;
        k.interp_out = interp_out;
        k.bezier_mode = mode;
        k.ease_in.clear();
        k.ease_out.clear();
        // 補間をハンドルで決め直すので、区間のイージングカーブは外す
        k.curve = None;
    }
}

/// 新しく作るレイヤーの種類。
#[derive(Copy, Clone)]
pub(crate) enum NewLayer {
    Solid,
    Text,
    Rect,
    Ellipse,
    Star,
    Null,
    Adjustment,
    Camera,
    Light(LightType),
}

/// プロパティへの操作。値は成分の配列で持つので、どの型のプロパティにも使える。
pub(crate) enum PropEdit {
    Set(Vec<f64>),
    /// ストップウォッチ: アニメーションの開始（現在値でキー）／解除（現在値で固定）。
    ToggleAnimation,
    AddKey,
    RemoveKey,
    Kind(KeyKind),
    /// エクスプレッションの設定（`None` で削除）。
    Expr(Option<String>),
    /// 時刻 `from` のキーを `to` へ動かす（グラフエディター）。
    MoveKey {
        from: Time,
        to: Time,
    },
}

impl PropEdit {
    pub fn coalesce(&self) -> bool {
        matches!(self, PropEdit::Set(_) | PropEdit::MoveKey { .. })
    }
}

fn apply_to<V: Animatable>(p: &mut Property<V>, t: Time, edit: &PropEdit) {
    match edit {
        PropEdit::Set(v) => {
            let cur = p.eval(t);
            if v.len() >= cur.dims() {
                p.set_value_at(t, cur.with_components(v));
            }
        }
        PropEdit::ToggleAnimation => {
            let v = p.eval(t);
            if p.is_animated() {
                p.keyframes.clear();
                p.value = v;
            } else {
                p.set_key(t, v);
            }
        }
        PropEdit::AddKey => {
            let v = p.eval(t);
            p.set_key(t, v);
        }
        PropEdit::RemoveKey => {
            p.remove_key(t);
        }
        PropEdit::Kind(kind) => {
            if let Some(i) = p.key_index_at(t) {
                kind.apply(&mut p.keyframes[i]);
            }
        }
        PropEdit::Expr(src) => p.expression = src.as_ref().map(|s| Expression { source: s.clone(), enabled: true }),
        PropEdit::MoveKey { from, to } => {
            if let Some(i) = p.key_index_at(*from) {
                if p.key_index_at(*to).is_none() {
                    p.keyframes[i].time = *to;
                    p.normalize_keys();
                }
            }
        }
    }
}

/// レイヤーの `path` のプロパティに操作を適用する。
pub(crate) fn apply_edit(layer: &mut Layer, path: &str, t: Time, edit: &PropEdit) {
    // タイムリマップは、無ければ作る（現在のソース時刻から始める）
    if path == "time_remap" && layer.time_remap.is_none() {
        layer.time_remap = Some(Property::constant((t - layer.start_time).seconds() * layer.speed));
    }
    visit_layer_mut(layer, &mut |p, m| {
        if p == path {
            match m {
                PropMut::F1(pr) => apply_to(pr, t, edit),
                PropMut::F2(pr) => apply_to(pr, t, edit),
                PropMut::F3(pr) => apply_to(pr, t, edit),
                PropMut::F4(pr) => apply_to(pr, t, edit),
                PropMut::Path(pr) => apply_to(pr, t, edit),
            }
        }
    });
}

/// 表示用に取り出したプロパティ 1 つぶんの状態。
pub(crate) struct Row {
    pub path: String,
    pub vals: Vec<f64>,
    pub is_path: bool,
    pub animated: bool,
    pub on_key: Option<KeyKind>,
    pub expr: Option<String>,
    /// キーフレームの時刻と補間の種類（昇順）。
    pub keys: Vec<(Time, KeyKind)>,
    /// キーごとの、次のキーまでの区間のイージングカーブ（`keys` と同じ並び）。
    pub curves: Vec<Option<EaseCurve>>,
}

fn snapshot<V: Animatable>(path: &str, p: &Property<V>, t: Time, is_path: bool) -> Row {
    let v = p.eval(t);
    Row {
        path: path.to_string(),
        vals: if is_path { Vec::new() } else { (0..v.dims()).map(|d| v.component(d)).collect() },
        is_path,
        animated: p.is_animated(),
        on_key: p.key_index_at(t).map(|i| KeyKind::of(&p.keyframes[i])),
        expr: p.expression.as_ref().map(|e| e.source.clone()),
        keys: p.keyframes.iter().map(|k| (k.time, KeyKind::of(k))).collect(),
        curves: p.keyframes.iter().map(|k| k.curve).collect(),
    }
}

pub(crate) fn collect_rows(layer: &Layer, t: Time) -> Vec<Row> {
    let mut rows = Vec::new();
    visit_layer(layer, &mut |path, r| {
        rows.push(match r {
            PropRef::F1(p) => snapshot(path, p, t, false),
            PropRef::F2(p) => snapshot(path, p, t, false),
            PropRef::F3(p) => snapshot(path, p, t, false),
            PropRef::F4(p) => snapshot(path, p, t, false),
            PropRef::Path(p) => snapshot(path, p, t, true),
        })
    });
    rows
}

/// プロパティのパスを、画面に出す名前にする。
pub(crate) fn path_label(layer: &Layer, path: &str) -> String {
    let fixed = match path {
        "transform.anchor" => "アンカーポイント",
        "transform.position" => "位置",
        "transform.scale" => "スケール",
        "transform.rotation" => "回転",
        "transform.opacity" => "不透明度",
        "transform.position_z" => "Z 位置",
        "transform.anchor_z" => "アンカー Z",
        "transform.rotation_x" => "X 回転",
        "transform.rotation_y" => "Y 回転",
        "time_remap" => "タイムリマップ（秒）",
        "audio.level" => "オーディオレベル（dB）",
        "camera.zoom" => "ズーム",
        "camera.point_of_interest" => "目標点",
        "camera.focus_distance" => "フォーカス距離",
        "camera.aperture" => "絞り",
        "light.intensity" => "強度（%）",
        "light.cone_angle" => "円錐角度",
        "light.cone_feather" => "円錐ぼかし",
        "text.path.first_margin" => "パス: 最初のマージン",
        _ => "",
    };
    if !fixed.is_empty() {
        return fixed.to_string();
    }
    let parts: Vec<&str> = path.split('.').collect();
    let word = |w: &str| -> String {
        match w {
            "path" => "パス",
            "feather" => "ぼかし",
            "opacity" => "不透明度",
            "expansion" => "拡張",
            "anchor" => "アンカー",
            "position" => "位置",
            "scale" => "スケール",
            "rotation" => "回転",
            "tracking" => "トラッキング",
            "fill_color" => "塗りの色",
            "stroke_color" => "線の色",
            "stroke_width" => "線幅",
            "blur" => "ブラー",
            "character_offset" => "文字オフセット",
            "position_z" => "Z 位置",
            "rotation_x" => "X 回転",
            "rotation_y" => "Y 回転",
            "start" => "開始",
            "end" => "終了",
            "offset" => "オフセット",
            "amount" => "量",
            "max_amount" => "最大量",
            "min_amount" => "最小量",
            "wiggles_per_second" => "ウィグル／秒",
            "correlation" => "相関",
            "size" => "サイズ",
            "roundness" => "角丸",
            "points" => "頂点数",
            "outer_radius" => "外半径",
            "inner_radius" => "内半径",
            "color" => "色",
            "width" => "線幅",
            "dash_offset" => "破線オフセット",
            "copies" => "コピー数",
            "end_opacity" => "終了不透明度",
            "ridges" => "折り返し",
            "amplitude" => "振幅",
            "wavelength" => "波長",
            "phase" => "位相",
            "detail" => "細かさ",
            "speed" => "速度",
            "radius" => "半径",
            "angle" => "角度",
            "center" => "中心",
            "transform" => "変形",
            "gradient_start" => "グラデーション開始",
            "gradient_end" => "グラデーション終了",
            other => other,
        }
        .to_string()
    };
    match parts.as_slice() {
        ["fx", i, name] => {
            let label = i
                .parse::<usize>()
                .ok()
                .and_then(|i| layer.effects.get(i))
                .and_then(|fx| effect_def(&fx.kind))
                .and_then(|d| d.params.iter().find(|p| p.name == *name))
                .map(|p| p.label);
            label.unwrap_or(name).to_string()
        }
        ["mask", _, name] => word(name),
        ["text", _, "sel", j, name] => format!("セレクター {}: {}", j.parse::<usize>().map(|v| v + 1).unwrap_or(0), word(name)),
        ["text", _, name] => word(name),
        ["shape", rest @ ..] => rest
            .iter()
            .map(|w| if w.parse::<usize>().is_ok() { format!("{}", w.parse::<usize>().unwrap() + 1) } else { word(w) })
            .collect::<Vec<_>>()
            .join(" › "),
        _ => path.to_string(),
    }
}

/// 値の入力欄（数値は青字のドラッグ欄、4 成分は色）。変更があれば操作を返す。`rtl` は右から並べるレイアウト用。
pub(crate) fn value_widgets(ui: &mut egui::Ui, row: &Row, rtl: bool) -> Option<PropEdit> {
    if row.is_path {
        ui.label(egui::RichText::new("パス").size(10.5).color(theme::TEXT_DIM));
        return None;
    }
    if row.vals.len() == 4 {
        let mut c = [row.vals[0] as f32, row.vals[1] as f32, row.vals[2] as f32, row.vals[3] as f32];
        return ui.color_edit_button_rgba_unmultiplied(&mut c).changed().then(|| PropEdit::Set(c.iter().map(|v| *v as f64).collect()));
    }
    let suffix = match row.path.rsplit('.').next().unwrap_or("") {
        "rotation" | "rotation_x" | "rotation_y" | "angle" | "cone_angle" => "°",
        "opacity" | "scale" | "end_opacity" => "%",
        _ => "",
    };
    let mut vals = row.vals.clone();
    let mut changed = false;
    ui.scope(|ui| {
        ui.visuals_mut().override_text_color = Some(theme::VALUE);
        ui.visuals_mut().widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
        ui.spacing_mut().item_spacing.x = 1.0;
        ui.spacing_mut().button_padding.x = 3.0;
        let n = vals.len();
        for k in 0..n {
            let i = if rtl { n - 1 - k } else { k };
            changed |= ui.add(egui::DragValue::new(&mut vals[i]).speed(0.5).max_decimals(2).suffix(suffix)).changed();
        }
    });
    changed.then_some(PropEdit::Set(vals))
}

/// フォントのファミリー名と、検索に使う文字列（ファミリー名と日本語名などの別名）。
fn font_list() -> &'static Vec<(String, String)> {
    static LIST: OnceLock<Vec<(String, String)>> = OnceLock::new();
    LIST.get_or_init(|| {
        znd_text::font_families_with_aliases()
            .into_iter()
            // 「.」で始まるのは OS が内部で使うフォント（macOS）なので一覧に出さない
            .filter(|(f, _)| !f.starts_with('.'))
            .map(|(f, aliases)| {
                let keys = std::iter::once(f.clone()).chain(aliases).collect::<Vec<_>>().join(" ");
                (f, keys)
            })
            .collect()
    })
}

/// アニメーターに付けられる属性（キー, 表示名）。足した直後は「変化なし」の値になる。
const ANIMATOR_PROPS: [(&str, &str); 13] = [
    ("position", "位置"),
    ("scale", "スケール"),
    ("rotation", "回転"),
    ("opacity", "不透明度"),
    ("anchor", "アンカーポイント"),
    ("tracking", "字間"),
    ("fill_color", "塗りの色"),
    ("stroke_color", "線の色"),
    ("stroke_width", "線の幅"),
    ("blur", "ブラー"),
    ("character_offset", "文字のずらし"),
    ("position_z", "Z 位置（3D）"),
    ("rotation_x", "X 回転（3D）"),
];

fn animator_prop_set(p: &AnimatorProps, key: &str) -> bool {
    match key {
        "position" => p.position.is_some(),
        "scale" => p.scale.is_some(),
        "rotation" => p.rotation.is_some(),
        "opacity" => p.opacity.is_some(),
        "anchor" => p.anchor.is_some(),
        "tracking" => p.tracking.is_some(),
        "fill_color" => p.fill_color.is_some(),
        "stroke_color" => p.stroke_color.is_some(),
        "stroke_width" => p.stroke_width.is_some(),
        "blur" => p.blur.is_some(),
        "character_offset" => p.character_offset.is_some(),
        "position_z" => p.position_z.is_some(),
        "rotation_x" => p.rotation_x.is_some(),
        _ => false,
    }
}

/// 属性を足す（変化なしの値で）・外す。
fn toggle_animator_prop(p: &mut AnimatorProps, key: &str, on: bool) {
    fn set<T: Animatable>(slot: &mut Option<Property<T>>, on: bool, neutral: T) {
        *slot = on.then(|| Property::constant(neutral));
    }
    match key {
        "position" => set(&mut p.position, on, [0.0, 0.0]),
        "scale" => set(&mut p.scale, on, [100.0, 100.0]),
        "rotation" => set(&mut p.rotation, on, 0.0),
        "opacity" => set(&mut p.opacity, on, 100.0),
        "anchor" => set(&mut p.anchor, on, [0.0, 0.0]),
        "tracking" => set(&mut p.tracking, on, 0.0),
        "fill_color" => set(&mut p.fill_color, on, [1.0, 1.0, 1.0, 1.0]),
        "stroke_color" => set(&mut p.stroke_color, on, [0.0, 0.0, 0.0, 1.0]),
        "stroke_width" => set(&mut p.stroke_width, on, 0.0),
        "blur" => set(&mut p.blur, on, 0.0),
        "character_offset" => set(&mut p.character_offset, on, 0.0),
        "position_z" => set(&mut p.position_z, on, 0.0),
        "rotation_x" => set(&mut p.rotation_x, on, 0.0),
        _ => {}
    }
}

const CLEARANCES: [(Clearance, &str); 4] = [
    (Clearance::Unconfirmed, "未確認"),
    (Clearance::Cleared, "許諾済み"),
    (Clearance::NotApplicable, "該当なし"),
    (Clearance::Denied, "不許可"),
];

impl App {
    // ---------- メニュー ----------

    pub(crate) fn menu_bar(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        egui::menu::bar(ui, |ui| {
            ui.spacing_mut().button_padding = egui::vec2(6.0, 2.0);
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.menu_button("ファイル", |ui| {
                if ui.button("新規（デモプロジェクト）").clicked() {
                    self.replace_project(ctx, demo::demo_project(), None);
                    self.status = "デモプロジェクトを開きました".into();
                    ui.close_menu();
                }
                if ui.button("開く…").clicked() {
                    ui.close_menu();
                    if let Some(p) = rfd::FileDialog::new().add_filter("znd プロジェクト", &["json"]).pick_file() {
                        self.open(ctx, &p);
                    }
                }
                if ui.add(egui::Button::new("保存").shortcut_text(self.shortcut_text(crate::keymap::Action::Save))).clicked() {
                    ui.close_menu();
                    self.save();
                }
                if ui.button("名前を付けて保存…").clicked() {
                    ui.close_menu();
                    self.save_as();
                }
                ui.separator();
                if ui
                    .button("素材を読み込む…")
                    .on_hover_text(
                        "動画・静止画・GIF/APNG・音声・PSD（レイヤー保持）・SVG/PDF/AI（ベクター保持）・3D モデル（glTF/OBJ/FBX/Alembic）",
                    )
                    .clicked()
                {
                    ui.close_menu();
                    self.import_footage_dialog(false);
                }
                if ui
                    .button("連番画像を読み込む…")
                    .on_hover_text("連番のうちの 1 枚を選ぶと、同じ規則のファイルをまとめて 1 本の動画にします")
                    .clicked()
                {
                    ui.close_menu();
                    self.import_footage_dialog(true);
                }
                if ui.button("歌詞を読み込む（LRC / SRT / ASS）…").clicked() {
                    ui.close_menu();
                    self.import_lyrics();
                }
                if ui.button("カメラを読み込む（.chan / Alembic）…").clicked() {
                    ui.close_menu();
                    self.import_camera();
                }
                ui.separator();
                if ui.button("欠落した素材を再リンク…").clicked() {
                    ui.close_menu();
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        let base = base_dir(self.path.as_deref());
                        let mut n = 0;
                        self.edit("素材の再リンク", false, |p| n = znd_media::manage::relink(p, &base, &dir));
                        let left = znd_media::manage::missing_footage(&self.project, &base).len();
                        self.status = format!("{n} 個をつなぎ直しました（残り {left} 個）");
                    }
                }
                if ui.button("ファイルを収集（権利情報付き）…").clicked() {
                    ui.close_menu();
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.status = match znd_media::manage::collect_files(&self.shared, &base_dir(self.path.as_deref()), None, &dir) {
                            Ok(r) => format!(
                                "{} 個の素材を {} に集めました{}",
                                r.copied,
                                dir.display(),
                                if r.missing.is_empty() {
                                    String::new()
                                } else {
                                    format!("（見つからない素材 {} 個）", r.missing.len())
                                }
                            ),
                            Err(e) => format!("収集に失敗しました: {e}"),
                        };
                    }
                }
                if ui.button("このコンポを書き出す（受け渡し用）…").clicked() {
                    ui.close_menu();
                    if let (Some(pkg), Some(path)) = (
                        collab::export_comp(&self.shared, &self.comp_id),
                        rfd::FileDialog::new().set_file_name("comp.znd.json").save_file(),
                    ) {
                        self.status = match io::save(&pkg, &path) {
                            Ok(()) => format!("コンポを {} に書き出しました", path.display()),
                            Err(e) => format!("書き出せません: {e}"),
                        };
                    }
                }
                if ui.button("コンポを読み込む…").clicked() {
                    ui.close_menu();
                    if let Some(path) = rfd::FileDialog::new().add_filter("znd プロジェクト", &["json"]).pick_file() {
                        match io::load(&path) {
                            Ok(pkg) => {
                                let mut id = None;
                                self.edit("コンポ読み込み", false, |p| id = collab::import_comp(p, &pkg));
                                self.status =
                                    id.map(|i| format!("コンポ「{i}」を読み込みました")).unwrap_or_else(|| "コンポがありません".into());
                            }
                            Err(e) => self.status = format!("読み込めません: {e}"),
                        }
                    }
                }
                ui.separator();
                if ui.button("書き出し…").clicked() {
                    ui.close_menu();
                    self.show.export = true;
                }
            });
            ui.menu_button("編集", |ui| {
                if ui.add_enabled(!self.undo.is_empty(), egui::Button::new("取り消す").shortcut_text(self.shortcut_text(crate::keymap::Action::Undo))).clicked() {
                    self.undo();
                    ui.close_menu();
                }
                if ui.add_enabled(!self.redo.is_empty(), egui::Button::new("やり直す").shortcut_text(self.shortcut_text(crate::keymap::Action::Redo))).clicked() {
                    self.redo();
                    ui.close_menu();
                }
                if ui.button("履歴…").clicked() {
                    self.show.history = true;
                    ui.close_menu();
                }
            });
            ui.menu_button("コンポジション", |ui| {
                self.comp_menu(ui);
                ui.separator();
                if ui.button("提出前チェックを実行").clicked() {
                    ui.close_menu();
                    self.start_check(ctx);
                    self.show.check = true;
                }
                if ui.button("クレジット文面をコピー").clicked() {
                    ui.close_menu();
                    ctx.copy_text(rights::credits(&self.shared, &self.comp_id));
                    self.status = "クレジット文面をクリップボードにコピーしました".into();
                }
                if ui.button("制作ログをコピー").clicked() {
                    ui.close_menu();
                    let sessions = self.path.as_ref().map(|p| log::read_sessions(p)).unwrap_or_default();
                    ctx.copy_text(log::summary_text(&self.shared, &sessions));
                    self.status = "制作ログをクリップボードにコピーしました".into();
                }
                if ui.button("メイキング用の分解を書き出す…").clicked() {
                    ui.close_menu();
                    self.export_breakdown();
                }
            });
            ui.menu_button("レイヤー", |ui| self.layer_menu(ui));
            ui.menu_button("エフェクト", |ui| self.effect_menu(ui));
            ui.menu_button("アニメーション", |ui| self.animation_menu(ctx, ui));
            ui.menu_button("プラグイン", |ui| self.plugins_menu(ui));
            ui.menu_button("表示", |ui| self.view_menu(ui));
            ui.menu_button("ウィンドウ", |ui| {
                ui.checkbox(&mut self.show.export, "書き出し");
                ui.checkbox(&mut self.show.check, "提出前チェック");
                ui.checkbox(&mut self.show.history, "履歴");
                ui.checkbox(&mut self.show.snapshots, "版（スナップショット）");
                ui.checkbox(&mut self.show.reviews, "レビューコメント");
                ui.checkbox(&mut self.show.graph, "グラフエディター");
                ui.checkbox(&mut self.show.script, "スクリプト");
                ui.checkbox(&mut self.show.plugins, "プラグイン");
                ui.separator();
                match crate::gpu_name() {
                    Some(name) => {
                        let mut on = crate::GPU_ENABLED.load(std::sync::atomic::Ordering::Relaxed);
                        if ui
                            .checkbox(&mut on, format!("エフェクトを GPU で処理（{name}）"))
                            .on_hover_text("対応するエフェクトを GPU で処理します。結果は CPU と同じです（差は 8bit の 1 段未満）")
                            .changed()
                        {
                            crate::GPU_ENABLED.store(on, std::sync::atomic::Ordering::Relaxed);
                            self.publish();
                        }
                    }
                    None => {
                        ui.weak("GPU は使えません（エフェクトは CPU で処理）");
                    }
                }
                ui.checkbox(&mut self.timelapse, "タイムラプス録画（30 秒おきに画面を保存）");
            });
            ui.menu_button("ヘルプ", |ui| {
                if ui.button("使用 OSS とライセンス").clicked() {
                    self.show.licenses = true;
                    ui.close_menu();
                }
                if ui.button("キーボードショートカット…").clicked() {
                    self.show.shortcuts = true;
                    ui.close_menu();
                }
            });

            widgets::vsep(ui);
            self.toolbar(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| self.status_area(ui));
        });
    }

    /// 進行中の処理（書き出し・提出前チェック）の（名前, 進捗, 中止フラグ）。
    pub(crate) fn running_task(&self) -> Option<(String, f32, std::sync::Arc<std::sync::atomic::AtomicBool>)> {
        self.export_task
            .as_ref()
            .map(|t| (t.label.clone(), t.fraction(), t.cancel.clone()))
            .or_else(|| self.check_task.as_ref().map(|t| (t.label.clone(), t.fraction(), t.cancel.clone())))
    }

    fn comp_menu(&mut self, ui: &mut egui::Ui) {
        if ui.button("新規コンポジション").clicked() {
            ui.close_menu();
            self.new_composition();
        }
        if ui.button("コンポジション設定…").on_hover_text("プロジェクトパネルの下段で編集します").clicked() {
            ui.close_menu();
            self.selected_footage = None;
        }
        ui.separator();
        if ui.button("ワークエリアの開始を現在の時刻に　B").clicked() {
            ui.close_menu();
            let end = self.work_area.map(|w| w.1).unwrap_or(self.total_frames()).max(self.frame + 1);
            self.work_area = Some((self.frame, end));
        }
        if ui.button("ワークエリアの終了を現在の時刻に　N").clicked() {
            ui.close_menu();
            let start = self.work_area.map(|w| w.0).unwrap_or(0).min(self.frame);
            self.work_area = Some((start, self.frame + 1));
        }
        if ui.add_enabled(self.work_area.is_some(), egui::Button::new("ワークエリアを解除")).clicked() {
            ui.close_menu();
            self.work_area = None;
        }
        if ui.button("現在の時刻にマーカーを追加　M").clicked() {
            ui.close_menu();
            self.add_marker("");
        }
        ui.separator();
        if ui.button("フレームを画像として保存…").clicked() {
            ui.close_menu();
            self.save_frame_image();
        }
    }

    /// 空のコンポジションを足して、そこへ切り替える（寸法とフレームレートは今のコンポを引き継ぐ）。
    pub(crate) fn new_composition(&mut self) {
        let shared = self.shared.clone();
        let (w, h, fr, dur) = shared.comp(&self.comp_id).map(|c| (c.width, c.height, c.frame_rate, c.duration)).unwrap_or((
            1920,
            1080,
            FrameRate::FPS_30,
            Time::from_seconds(10.0),
        ));
        let id = shared.fresh_id("comp");
        let name = format!("コンポ {}", shared.compositions.len() + 1);
        let comp = Composition::new(id.clone(), name, w, h, fr, dur);
        self.edit("新規コンポジション", false, |p| p.compositions.push(comp));
        self.switch_comp(CompId::new(id));
    }

    pub(crate) fn switch_comp(&mut self, id: CompId) {
        if id != self.comp_id {
            self.comp_id = id;
            self.selected = None;
            self.selected_keys.clear();
            self.key_target = None;
            self.frame = 0;
            self.stop();
            self.audio_sig = 0;
        }
    }

    /// 親を設定する。循環する場合は元に戻す。
    pub(crate) fn set_parent(&mut self, id: &LayerId, parent: Option<LayerId>) {
        let set = parent.is_some();
        self.edit_layer("親", false, id, |l| l.parent = parent);
        if set && !self.project.validate().is_empty() {
            self.undo();
            self.redo.clear();
            self.status = "親子関係が循環するため設定できません".into();
        }
    }

    /// 選択中のレイヤーにエフェクトを足す。
    pub(crate) fn add_effect(&mut self, kind: &str) {
        let shared = self.shared.clone();
        let Some(comp) = shared.comp(&self.comp_id) else { return };
        let Some(layer) = self.selected.as_ref().and_then(|id| comp.layer(id)) else {
            self.status = "エフェクトを掛けるレイヤーを先に選択してください".into();
            return;
        };
        let (w, h) = shared.source_size(&layer.kind).unwrap_or((comp.width as f64, comp.height as f64));
        if let Some(fx) = EffectInstance::new(kind, w, h) {
            let id = layer.id.clone();
            self.status = format!("「{}」に {} を追加しました", layer.name, fx.name);
            self.edit_layer("エフェクト追加", false, &id, |l| l.effects.push(fx));
        }
    }

    /// 分類ごとのエフェクト一覧（組み込みと、読み込み済みのプラグイン）。
    pub(crate) fn effect_menu(&mut self, ui: &mut egui::Ui) {
        if self.selected.is_none() {
            ui.label(egui::RichText::new("レイヤーを選択すると追加できます").size(10.5).color(theme::TEXT_DIM));
            ui.separator();
        }
        let effects = all_effects();
        // 検索中は分類をたどらずに、一致したものを並べる
        let mut pick = widgets::pick_search(ui, ui.make_persistent_id("effect-menu"), effects.len());
        if pick.searching() {
            let found: Vec<_> = effects.iter().filter(|d| pick.matches(&format!("{} {}", d.label, d.category))).collect();
            if found.is_empty() {
                ui.weak("一致するエフェクトがありません");
            }
            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                for def in found {
                    let enter = pick.take_enter();
                    let r = ui.button(def.label).on_hover_text(def.category);
                    if r.clicked() || enter {
                        ui.close_menu();
                        self.add_effect(def.kind);
                    }
                }
            });
            return;
        }
        let mut categories: Vec<&str> = Vec::new();
        for d in &effects {
            if !categories.contains(&d.category) {
                categories.push(d.category);
            }
        }
        for cat in categories {
            ui.menu_button(cat, |ui| {
                for def in effects.iter().filter(|d| d.category == cat) {
                    if ui.button(def.label).clicked() {
                        ui.close_menu();
                        self.add_effect(def.kind);
                    }
                }
            });
        }
    }

    fn animation_menu(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        self.key_menu(ui);
        if ui.add_enabled(self.selected.is_some(), egui::Button::new("選択レイヤーのキーフレームをすべて選択")).clicked() {
            ui.close_menu();
            self.select_all_keys();
        }
        ui.separator();
        if ui.button("グラフエディター").clicked() {
            ui.close_menu();
            self.show.graph = true;
        }
        ui.checkbox(&mut self.only_animated, "動きのあるプロパティだけを表示");
        ui.checkbox(&mut self.snap, "スナップ");
        ui.separator();
        ui.menu_button("オーディオから生成", |ui| self.audio_menu(ctx, ui));
        if ui.button("ビートに合わせる…").on_hover_text("ビートやマーカーの位置にレイヤーを並べる・パルスのキーを打つ").clicked() {
            ui.close_menu();
            self.show.beatfit = true;
        }
    }

    fn view_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("拡大率", |ui| {
            ui.radio_value(&mut self.zoom, 0.0, "全体表示");
            for z in [0.25, 0.5, 1.0, 2.0, 4.0] {
                ui.radio_value(&mut self.zoom, z, format!("{:.0}%", z * 100.0));
            }
        });
        ui.menu_button("解像度", |ui| {
            for ds in [1, 2, 4] {
                ui.radio_value(&mut self.downsample, ds, crate::resolution_label(ds));
            }
        });
        ui.separator();
        ui.checkbox(&mut self.show_guides, "セーフマージン");
        ui.checkbox(&mut self.show_grid, "グリッド");
        ui.checkbox(&mut self.screening, "上映プレビュー");
        ui.separator();
        ui.checkbox(&mut self.hide_shy, "シャイのレイヤーを隠す");
        if ui.button("タイムライン全体を表示").clicked() {
            ui.close_menu();
            self.tl_zoom = 1.0;
            self.tl_scroll = 0.0;
        }
    }

    /// 新しいレイヤーを作って選択する（メニューとツールバーで共通）。
    pub(crate) fn create_layer(&mut self, what: NewLayer) {
        let Some((w, h, dur)) = self.shared.comp(&self.comp_id).map(|c| (c.width, c.height, c.duration)) else { return };
        let shape = |items: Vec<ShapeItem>| LayerKind::Shape { items };
        match what {
            NewLayer::Solid => self.add_layer("平面追加", "平面", LayerKind::Solid { width: w, height: h, color: [0.5, 0.5, 0.5] }),
            NewLayer::Text => {
                let mut doc = TextDocument::new("テキスト", 96.0);
                doc.justify = Justify::Center;
                self.add_layer(
                    "テキスト追加",
                    "テキスト",
                    LayerKind::Text { document: doc, animators: Vec::new(), path: None, per_char_3d: false, text_expression: None },
                );
            }
            NewLayer::Rect => self.add_layer(
                "シェイプ追加",
                "シェイプ",
                shape(vec![ShapeItem::rect(300.0, 200.0), ShapeItem::fill([1.0, 1.0, 1.0, 1.0])]),
            ),
            NewLayer::Ellipse => self.add_layer(
                "シェイプ追加",
                "シェイプ",
                shape(vec![ShapeItem::ellipse(300.0, 300.0), ShapeItem::fill([1.0, 1.0, 1.0, 1.0])]),
            ),
            NewLayer::Star => self.add_layer(
                "シェイプ追加",
                "シェイプ",
                shape(vec![ShapeItem::star(5.0, 200.0, 90.0), ShapeItem::fill([1.0, 0.85, 0.2, 1.0])]),
            ),
            NewLayer::Null => self.add_layer("ヌル追加", "ヌル", LayerKind::Null),
            NewLayer::Adjustment => self.add_layer("調整レイヤー追加", "調整レイヤー", LayerKind::Adjustment),
            NewLayer::Camera => {
                let id = self.shared.fresh_id("camera");
                let mut cam = camera_import::default_camera(&id, w, h, dur);
                if let Some(comp) = self.shared.comp(&self.comp_id) {
                    crate::place_at(&mut cam, self.playhead_time(comp), dur, &self.shared);
                }
                self.edit_comp("カメラ追加", false, |c| c.layers.insert(0, cam));
                self.selected = Some(LayerId::new(id));
            }
            NewLayer::Light(kind) => {
                let name = match kind {
                    LightType::Point => "ライト（ポイント）",
                    LightType::Spot => "ライト（スポット）",
                    LightType::Parallel => "ライト（平行）",
                    LightType::Ambient => "ライト（アンビエント）",
                };
                let light = LayerKind::Light {
                    light: kind,
                    color: [1.0, 1.0, 1.0],
                    intensity: Property::constant(100.0),
                    cone_angle: Property::constant(90.0),
                    cone_feather: Property::constant(50.0),
                    casts_shadows: false,
                    shadow_darkness: 100.0,
                    shadow_diffusion: 0.0,
                };
                self.add_layer("ライト追加", name, light);
                if let Some(id) = self.selected.clone() {
                    self.edit_layer("ライト追加", true, &id, |l| l.transform.position_z = Property::constant(-500.0));
                }
            }
        }
    }

    fn layer_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("新規", |ui| {
            for (label, what) in [
                ("平面", NewLayer::Solid),
                ("テキスト", NewLayer::Text),
                ("シェイプ（矩形）", NewLayer::Rect),
                ("シェイプ（楕円）", NewLayer::Ellipse),
                ("シェイプ（スター）", NewLayer::Star),
                ("ヌル", NewLayer::Null),
                ("調整レイヤー", NewLayer::Adjustment),
                ("カメラ", NewLayer::Camera),
                ("ライト（ポイント）", NewLayer::Light(LightType::Point)),
                ("ライト（スポット）", NewLayer::Light(LightType::Spot)),
                ("ライト（平行）", NewLayer::Light(LightType::Parallel)),
                ("ライト（アンビエント）", NewLayer::Light(LightType::Ambient)),
            ] {
                if ui.button(label).clicked() {
                    ui.close_menu();
                    self.create_layer(what);
                }
            }
        });
        ui.separator();
        let selected = self.selected.clone();
        let has = selected.is_some();
        if ui.add_enabled(has, egui::Button::new("複製")).clicked() {
            ui.close_menu();
            if let Some(id) = selected.clone() {
                self.duplicate_layer(&id);
            }
        }
        if ui.add_enabled(has, egui::Button::new("削除")).clicked() {
            ui.close_menu();
            if let Some(id) = selected.clone() {
                self.delete_layer(&id);
            }
        }
        if ui.add_enabled(has, egui::Button::new("現在の時刻で分割")).on_hover_text("映像と音声を同じ位置で切ります").clicked()
        {
            ui.close_menu();
            if let Some(id) = selected.clone() {
                let t = self.frame_rate().frame_to_time(self.frame);
                let new_id = LayerId::new(self.shared.fresh_id("layer"));
                self.edit_comp("レイヤー分割", false, |c| {
                    if let Some(i) = c.layer_index(&id) {
                        if t > c.layers[i].in_point && t < c.layers[i].out_point {
                            let mut tail = c.layers[i].clone();
                            tail.id = new_id;
                            tail.in_point = t;
                            c.layers[i].out_point = t;
                            c.layers.insert(i, tail);
                        }
                    }
                });
            }
        }
        if ui.add_enabled(has, egui::Button::new("上へ")).clicked() {
            if let Some(id) = selected.clone() {
                self.edit_comp("重ね順", false, |c| {
                    if let Some(i) = c.layer_index(&id).filter(|i| *i > 0) {
                        c.layers.swap(i, i - 1);
                    }
                });
            }
        }
        if ui.add_enabled(has, egui::Button::new("下へ")).clicked() {
            if let Some(id) = selected.clone() {
                self.edit_comp("重ね順", false, |c| {
                    if let Some(i) = c.layer_index(&id).filter(|i| i + 1 < c.layers.len()) {
                        c.layers.swap(i, i + 1);
                    }
                });
            }
        }
        if ui.add_enabled(has, egui::Button::new("プリコンポーズ")).clicked() {
            ui.close_menu();
            if let Some(id) = selected.clone() {
                self.precompose(&id);
            }
        }
        let is_text = selected
            .as_ref()
            .and_then(|id| self.shared.comp(&self.comp_id).and_then(|c| c.layer(id)))
            .is_some_and(|l| matches!(l.kind, LayerKind::Text { .. }));
        if ui.add_enabled(is_text, egui::Button::new("テキストをシェイプに変換")).clicked() {
            ui.close_menu();
            if let Some(id) = selected.clone() {
                let t = self.frame_rate().frame_to_time(self.frame);
                self.edit_layer("アウトライン化", false, &id, |l| {
                    if let LayerKind::Text { document, .. } = &l.kind {
                        let fill = document.fill.unwrap_or([1.0, 1.0, 1.0, 1.0]);
                        let mut items: Vec<ShapeItem> = znd_text::outlines(document, t).into_iter().map(ShapeItem::path).collect();
                        items.push(ShapeItem::fill(fill));
                        l.kind = LayerKind::Shape { items };
                    }
                });
            }
        }
        if ui.add_enabled(has, egui::Button::new("削除")).clicked() {
            ui.close_menu();
            if let Some(id) = selected {
                // このレイヤーを親・マット・エフェクトの参照先にしている設定も外す（参照切れを残さない）
                self.edit_comp("レイヤー削除", false, |c| {
                    c.layers.retain(|l| l.id != id);
                    for l in &mut c.layers {
                        if l.parent.as_ref() == Some(&id) {
                            l.parent = None;
                        }
                        if l.track_matte.as_ref().is_some_and(|m| m.layer == id) {
                            l.track_matte = None;
                        }
                        for fx in &mut l.effects {
                            for v in fx.params.values_mut() {
                                if matches!(v, ParamValue::Layer(Some(r)) if *r == id) {
                                    *v = ParamValue::Layer(None);
                                }
                            }
                        }
                    }
                });
                self.selected = None;
            }
        }
    }

    fn precompose(&mut self, id: &LayerId) {
        let shared = self.shared.clone();
        let Some(comp) = shared.comp(&self.comp_id) else { return };
        let Some(layer) = comp.layer(id).cloned() else { return };
        let new_comp = CompId::new(shared.fresh_id("comp"));
        let mut nested =
            Composition::new(new_comp.0.clone(), format!("{} コンポ", layer.name), comp.width, comp.height, comp.frame_rate, comp.duration);
        // 親とマットは元のコンポに残る（中へ持っていくと参照が切れる）
        let mut inner = layer.clone();
        inner.parent = None;
        inner.track_matte = None;
        nested.layers.push(inner);
        let mut outer = Layer::new(
            layer.id.0.clone(),
            layer.name.clone(),
            LayerKind::Precomp { comp: new_comp },
            Transform::centered(comp.width as f64, comp.height as f64, comp.width as f64, comp.height as f64),
            comp.duration,
        );
        outer.parent = layer.parent.clone();
        outer.track_matte = layer.track_matte.clone();
        outer.in_point = layer.in_point;
        outer.out_point = layer.out_point;
        let (cid, lid) = (self.comp_id.clone(), id.clone());
        self.edit("プリコンポーズ", false, |p| {
            if let Some(l) = p.comp_mut(&cid).and_then(|c| c.layer_mut(&lid)) {
                *l = outer;
            }
            p.compositions.push(nested);
        });
    }

    pub(crate) fn audio_menu(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let shared = self.shared.clone();
        let Some(comp) = shared.comp(&self.comp_id) else { return };
        // 解析の対象: 選択中のレイヤーに音があればそれ、無ければコンポ全体のミックス
        let source = self.selected.clone().filter(|id| comp.layer(id).is_some_and(|l| znd_audio::layer_footage(&shared, l).is_some()));
        let label =
            source.as_ref().and_then(|id| comp.layer(id)).map(|l| format!("「{}」", l.name)).unwrap_or_else(|| "コンポ全体の音".into());
        ui.weak(format!("解析の対象: {label}"));
        let mono = |app: &App| -> Result<Vec<f32>, String> {
            let stereo: Vec<f32> = match &source {
                Some(id) => {
                    let mixer = Mixer::new(&shared, &*app.engine.clips);
                    mixer.mix_layer(&app.comp_id, id, 0, mixer.comp_frames(comp))?
                }
                None => app.audio_buf.to_vec(),
            };
            Ok(stereo.chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5).collect())
        };
        for (name, sensitivity) in [
            ("ビートを検出してマーカーを打つ（控えめ）", 0.3),
            ("ビートを検出してマーカーを打つ（標準）", 0.5),
            ("ビートを検出してマーカーを打つ（細かく）", 0.8),
        ] {
            if ui.button(name).clicked() {
                ui.close_menu();
                match mono(self) {
                    Ok(m) => {
                        let fr = comp.frame_rate;
                        let onsets = detect_onsets(&m, sensitivity);
                        let n = onsets.len();
                        self.edit_comp("ビート検出", false, |c| {
                            for t in onsets {
                                c.markers.push(Marker {
                                    time: beat::nearest_frame(Time::from_seconds(t), fr),
                                    duration: Time::ZERO,
                                    comment: String::new(),
                                });
                            }
                            c.markers.sort_by_key(|m| m.time);
                            c.markers.dedup_by_key(|m| m.time);
                        });
                        self.status = format!("{n} 個のビートを検出しました");
                    }
                    Err(e) => self.status = format!("解析できません: {e}"),
                }
            }
        }
        ui.separator();
        let target = self.selected.clone();
        for (name, band) in [
            ("低域の音量 → 選択レイヤーのスケール", Band::LOW),
            ("中域の音量 → 選択レイヤーのスケール", Band::MID),
            ("高域の音量 → 選択レイヤーのスケール", Band::HIGH),
        ] {
            if ui
                .add_enabled(target.is_some(), egui::Button::new(name))
                .on_hover_text("音量をフレームごとのキーフレームにして、スケール 100〜150% に割り当てます")
                .clicked()
            {
                ui.close_menu();
                if let (Ok(m), Some(id)) = (mono(self), target.clone()) {
                    let keys = band_keyframes(&m, band, comp.frame_rate, Time::ZERO, comp.duration);
                    self.edit_layer("音量をキーフレーム化", false, &id, |l| {
                        l.transform.scale = Property::constant([100.0, 100.0]).with_keys(
                            keys.keyframes
                                .iter()
                                .map(|k| {
                                    let s = 100.0 + (k.value * 2.0).min(100.0) * 0.5;
                                    Keyframe::linear(k.time, [s, s])
                                })
                                .collect(),
                        );
                    });
                }
            }
        }
        ui.separator();
        if ui
            .button("空コメントのマーカーから BPM を推定（タップ入力）")
            .on_hover_text("再生しながら M キーで拍を叩いてから実行します")
            .clicked()
        {
            ui.close_menu();
            let taps: Vec<Time> = comp.markers.iter().filter(|m| m.comment.is_empty()).map(|m| m.time).collect();
            match beat::grid_from_taps(&taps) {
                Some(g) => {
                    self.status = format!("BPM {:.1}（オフセット {:.3} 秒）を設定しました", g.bpm, g.offset.seconds());
                    self.edit_comp("ビートグリッド", false, |c| c.beat_grid = Some(g));
                }
                None => self.status = "マーカーが 2 個以上必要です".into(),
            }
        }
        if ui.button("マーカーをすべて削除").clicked() {
            ui.close_menu();
            self.edit_comp("マーカー削除", false, |c| c.markers.clear());
        }
        let _ = ctx;
    }

    fn import_lyrics(&mut self) {
        let Some(path) = rfd::FileDialog::new().add_filter("歌詞・字幕", &["lrc", "srt", "ass", "ssa"]).pick_file() else { return };
        let parsed =
            std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|s| lyrics::parse_lyrics(&path.to_string_lossy(), &s));
        match parsed {
            Ok(lines) => {
                let id = self.comp_id.clone();
                let mut n = 0;
                self.edit("歌詞読み込み", false, |p| {
                    if let Some(i) = p.compositions.iter().position(|c| c.id == id) {
                        n = lyrics::add_lyric_layers(p, i, &lines, 72.0);
                    }
                });
                self.status = format!("歌詞 {} 行をテキストレイヤーとマーカーにしました", n);
            }
            Err(e) => self.status = format!("歌詞を読み込めません: {e}"),
        }
    }

    fn import_camera(&mut self) {
        let Some(path) = rfd::FileDialog::new().add_filter("カメラデータ", &["chan", "abc"]).pick_file() else { return };
        let shared = self.shared.clone();
        let Some(comp) = shared.comp(&self.comp_id) else { return };
        let parsed = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("abc")) {
            znd_media::abc_camera_layer(&path, comp.width, comp.height, comp.frame_rate, 100.0).map_err(|e| e.to_string())
        } else {
            std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|s| camera_import::parse_chan(&s, comp.width, comp.height, comp.frame_rate, 100.0, 39.6))
        };
        match parsed {
            Ok(mut cam) => {
                cam.id = LayerId::new(shared.fresh_id("camera"));
                cam.out_point = cam.out_point.max(comp.duration);
                self.selected = Some(cam.id.clone());
                self.edit_comp("カメラ読み込み", false, |c| c.layers.insert(0, cam));
                self.status = "カメラを読み込みました（1 単位 = 100px）".into();
            }
            Err(e) => self.status = format!("カメラを読み込めません: {e}"),
        }
    }

    fn export_breakdown(&mut self) {
        let Some(dir) = rfd::FileDialog::new().pick_folder() else { return };
        let (project, comp, frame, footage) = (self.shared.clone(), self.comp_id.clone(), self.frame, self.engine.footage.clone());
        let t = self.frame_rate().frame_to_time(frame);
        let exprs = znd_script::ExprEngine::new();
        let ctx = znd_media::ExportContext {
            project: &project,
            footage: &*footage,
            clips: None,
            resolver: Some(&exprs),
            base_dir: Some(base_dir(self.path.as_deref())),
            accel: crate::gpu(),
            composite: crate::gpu_composite(),
        };
        self.status = match znd_media::review::export_breakdown(&ctx, &comp, t, &dir, "making") {
            Ok(files) => format!("{} 枚の分解画像を {} に書き出しました", files.len(), dir.display()),
            Err(e) => format!("書き出せません: {e}"),
        };
    }

    // ---------- パネルの中身 ----------

    pub(crate) fn comp_settings(&mut self, ui: &mut egui::Ui, comp: &Composition) {
        let mut name = comp.name.clone();
        if ui.add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY)).changed() {
            self.edit_comp("コンポ名", true, |c| c.name = name);
        }
        ui.horizontal_wrapped(|ui| {
            let (mut w, mut h) = (comp.width, comp.height);
            ui.label("解像度");
            let cw = ui.add(egui::DragValue::new(&mut w).range(2..=16384)).changed();
            let ch = ui.add(egui::DragValue::new(&mut h).range(2..=16384)).changed();
            if cw || ch {
                self.edit_comp("解像度", true, |c| {
                    c.width = w;
                    c.height = h;
                });
            }
            widgets::combo(ui, "fps", 0.0, format!("{:.3} fps", comp.frame_rate.fps()), FrameRate::PRESETS.len(), |ui, pick| {
                for fr in FrameRate::PRESETS {
                    if pick.item(ui, fr == comp.frame_rate, format!("{:.3}", fr.fps())) {
                        self.edit_comp("フレームレート", false, |c| c.frame_rate = fr);
                    }
                }
            });
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("尺（秒）");
            let mut secs = comp.duration.seconds();
            if ui.add(egui::DragValue::new(&mut secs).speed(0.1).range(0.1..=3600.0)).changed() {
                self.edit_comp("尺", true, |c| c.duration = Time::from_seconds(secs));
            }
            ui.label("背景色");
            let mut bg = comp.background;
            if ui.color_edit_button_rgb(&mut bg).changed() {
                self.edit_comp("背景色", true, |c| c.background = bg);
            }
        });
        ui.horizontal_wrapped(|ui| {
            let mut mb = comp.motion_blur;
            let mut changed = ui.checkbox(&mut mb.enabled, "モーションブラー").changed();
            changed |= ui.add(egui::DragValue::new(&mut mb.shutter_angle).range(1.0..=720.0).prefix("角度 ")).changed();
            changed |= ui.add(egui::DragValue::new(&mut mb.shutter_phase).range(-360.0..=360.0).prefix("位相 ")).changed();
            changed |= ui.add(egui::DragValue::new(&mut mb.samples).range(2..=64).prefix("サンプル ")).changed();
            if changed {
                self.edit_comp("モーションブラー", true, |c| c.motion_blur = mb);
            }
        });
        ui.horizontal_wrapped(|ui| {
            let mut on = comp.beat_grid.is_some();
            let mut g = comp.beat_grid.unwrap_or_else(|| BeatGrid::new(120.0));
            let mut changed = ui.checkbox(&mut on, "ビートグリッド").changed();
            changed |= ui.add_enabled(on, egui::DragValue::new(&mut g.bpm).speed(0.1).range(20.0..=400.0).prefix("BPM ")).changed();
            changed |= ui.add_enabled(on, egui::DragValue::new(&mut g.beats_per_bar).range(1..=16).suffix(" 拍子")).changed();
            changed |= ui.add_enabled(on, egui::DragValue::new(&mut g.subdivision).range(1..=16).prefix("分割 ")).changed();
            let mut off = g.offset.seconds();
            if ui.add_enabled(on, egui::DragValue::new(&mut off).speed(0.001).prefix("開始 ").suffix(" 秒")).changed() {
                g.offset = Time::from_seconds(off);
                changed = true;
            }
            if changed {
                self.edit_comp("ビートグリッド", true, |c| c.beat_grid = on.then_some(g));
            }
        });
        ui.horizontal_wrapped(|ui| {
            match &comp.locked_by {
                None => {
                    if ui
                        .button("このコンポをロック")
                        .on_hover_text("他の人が編集できないようにします（プロジェクトファイルに記録）")
                        .clicked()
                    {
                        let user = self.user.clone();
                        self.edit_comp("ロック", false, |c| c.locked_by = Some(user));
                    }
                }
                Some(owner) if owner == &self.user => {
                    if ui.button("ロックを解除").clicked() {
                        self.edit_comp("ロック解除", false, |c| c.locked_by = None);
                    }
                }
                Some(owner) => {
                    ui.colored_label(theme::ERROR, format!("{owner} がロック中"));
                }
            }
            ui.label("名前");
            ui.add(egui::TextEdit::singleline(&mut self.user).desired_width(90.0));
        });
        ui.separator();
        let s = &self.shared.settings.clone();
        ui.horizontal_wrapped(|ui| {
            let mut linear = s.linear_blending;
            if ui.checkbox(&mut linear, "リニアライト合成").changed() {
                self.edit("合成設定", false, |p| p.settings.linear_blending = linear);
            }
            widgets::combo(ui, "depth", 0.0, match s.bit_depth {
                    BitDepth::B8 => "8bpc",
                    BitDepth::B16 => "16bpc",
                    BitDepth::F32 => "32bpc",
                }, 3, |ui, pick| {
                    for (d, label) in [(BitDepth::B8, "8bpc"), (BitDepth::B16, "16bpc"), (BitDepth::F32, "32bpc（浮動小数）")] {
                        if pick.item(ui, d == s.bit_depth, label) {
                            self.edit("色深度", false, |p| p.settings.bit_depth = d);
                        }
                    }
                });
        });
        const SPACES: [(ColorSpace, &str); 4] = [
            (ColorSpace::Srgb, "sRGB"),
            (ColorSpace::Rec709, "Rec.709"),
            (ColorSpace::DisplayP3, "Display P3"),
            (ColorSpace::AcesCg, "ACEScg"),
        ];
        let name_of = |c: ColorSpace| SPACES.iter().find(|(s, _)| *s == c).map(|(_, n)| *n).unwrap_or("");
        ui.horizontal_wrapped(|ui| {
            ui.label("作業色空間");
            widgets::combo(ui, "working", 0.0, name_of(s.working_space), SPACES.len(), |ui, pick| {
                for (space, label) in SPACES {
                    if pick.item(ui, space == s.working_space, label) {
                        self.edit("作業色空間", false, |p| p.settings.working_space = space);
                    }
                }
            });
            ui.label("出力");
            widgets::combo(ui, "output", 0.0, name_of(s.output_space), SPACES.len(), |ui, pick| {
                for (space, label) in SPACES {
                    if pick.item(ui, space == s.output_space, label) {
                        self.edit("出力色空間", false, |p| p.settings.output_space = space);
                    }
                }
            });
        });
    }

    pub(crate) fn footage_panel(&mut self, ui: &mut egui::Ui, item: &FootageItem) {
        ui.label(format!("{}　{}×{}", item.name, item.width, item.height));
        ui.add(egui::Label::new(egui::RichText::new(&item.path).size(10.0).color(theme::TEXT_DIM)).truncate());
        if item.vfr {
            ui.colored_label(theme::WARN, "可変フレームレート素材（固定レートに適合して読み込み）");
        }
        let id = item.id.clone();
        ui.horizontal_wrapped(|ui| {
            if ui.button("レイヤーとして追加").clicked() {
                let kind =
                    if item.width == 0 { LayerKind::Audio { footage: id.clone() } } else { LayerKind::Footage { footage: id.clone() } };
                self.add_layer("レイヤー追加", &item.name, kind);
            }
            if item.frame_rate.is_some()
                && ui.button("プロキシを生成").on_hover_text("1/2 解像度の代替ファイルを作り、プレビューを軽くします").clicked()
            {
                self.status = match znd_media::make_proxy(&base_dir(self.path.as_deref()), item, 2) {
                    Ok((p, w, h)) => {
                        self.engine.footage.set_proxy(&item.id, p, w, h);
                        format!("プロキシを生成しました（{w}x{h}）")
                    }
                    Err(e) => format!("プロキシを生成できません: {e}"),
                };
            }
        });
        ui.add_space(4.0);
        ui.label(egui::RichText::new("権利情報").size(10.5).color(theme::TEXT_DIM));
        let mut r = item.rights.clone();
        let mut changed = false;
        egui::Grid::new("rights").num_columns(2).show(ui, |ui| {
            for (label, field) in
                [("作者", &mut r.author), ("入手元 URL", &mut r.source_url), ("ライセンス", &mut r.license), ("許諾日", &mut r.cleared_on)]
            {
                ui.label(label);
                changed |= ui.text_edit_singleline(field).changed();
                ui.end_row();
            }
            for (label, field) in [
                ("著作権", &mut r.copyright),
                ("著作隣接権・原盤権", &mut r.neighboring),
                ("肖像権", &mut r.portrait),
                ("商標", &mut r.trademark),
            ] {
                ui.label(label);
                let current = CLEARANCES.iter().find(|(c, _)| c == field).map(|(_, l)| *l).unwrap_or("");
                widgets::combo(ui, label, 0.0, current, CLEARANCES.len(), |ui, pick| {
                    for (c, l) in CLEARANCES {
                        if pick.item(ui, *field == c, l) {
                            *field = c;
                            changed = true;
                        }
                    }
                });
                ui.end_row();
            }
            ui.label("利用条件メモ");
            changed |= ui.text_edit_multiline(&mut r.notes).changed();
            ui.end_row();
        });
        if ui.button("自作素材（すべて該当なし）にする").clicked() {
            r.copyright = Clearance::NotApplicable;
            r.neighboring = Clearance::NotApplicable;
            r.portrait = Clearance::NotApplicable;
            r.trademark = Clearance::NotApplicable;
            changed = true;
        }
        if changed {
            self.edit("権利情報", true, |p| {
                if let Some(f) = p.footage.iter_mut().find(|f| f.id == id) {
                    f.rights = r;
                }
            });
        }
    }

    /// プロパティ 1 行（ストップウォッチ・名前・値・キー・エクスプレッション）。操作があれば編集として適用する。
    fn prop_row(&mut self, ui: &mut egui::Ui, layer: &Layer, row: &Row, t: Time) {
        let label = path_label(layer, &row.path);
        let mut edit: Option<PropEdit> = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.spacing_mut().interact_size.y = 18.0;
            if widgets::small_icon(ui, Icon::Stopwatch, row.animated, "ストップウォッチ: アニメーションのオン／オフ").clicked()
            {
                edit = Some(PropEdit::ToggleAnimation);
            }
            let (r, _) = ui.allocate_exact_size(egui::vec2(92.0, 18.0), egui::Sense::hover());
            ui.painter().with_clip_rect(r).text(
                r.left_center(),
                egui::Align2::LEFT_CENTER,
                &label,
                egui::FontId::proportional(11.5),
                theme::TEXT,
            );
            if let Some(e) = value_widgets(ui, row, false) {
                edit = Some(e);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !row.is_path {
                    let has_error = self.expr_errors.1.iter().any(|e| e.layer == layer.name && e.path == row.path);
                    let open = self.expr_open.as_deref() == Some(&row.path);
                    let (r, resp) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
                    let color = if has_error {
                        theme::ERROR
                    } else if row.expr.is_some() {
                        theme::VALUE
                    } else if resp.hovered() {
                        egui::Color32::WHITE
                    } else {
                        theme::TEXT_OFF
                    };
                    if open {
                        ui.painter().rect_filled(r, 2.0, theme::BG_HOVER);
                    }
                    icons::paint_sized(ui.painter(), r, Icon::Expr, color, 14.0);
                    if resp.on_hover_text("エクスプレッション").clicked() {
                        if open {
                            self.expr_open = None;
                        } else {
                            self.expr_open = Some(row.path.clone());
                            self.expr_buf = row.expr.clone().unwrap_or_default();
                        }
                    }
                }
                if row.animated {
                    if !row.is_path && widgets::small_icon(ui, Icon::Graph, false, "グラフエディターで開く").clicked() {
                        self.graph_path = Some(row.path.clone());
                        self.show.graph = true;
                    }
                    let on = row.on_key.is_some();
                    let resp = widgets::small_icon(
                        ui,
                        if on { Icon::DiamondFill } else { Icon::Diamond },
                        on,
                        "この時刻のキーフレームを追加／削除（右クリックで補間の種類）",
                    );
                    if resp.clicked() {
                        edit = Some(if on { PropEdit::RemoveKey } else { PropEdit::AddKey });
                    }
                    if let Some(kind) = row.on_key {
                        resp.context_menu(|ui| {
                            for k in KeyKind::ALL {
                                if ui.selectable_label(k == kind, k.label()).clicked() {
                                    edit = Some(PropEdit::Kind(k));
                                    ui.close_menu();
                                }
                            }
                        });
                    }
                }
            });
        });
        if self.expr_open.as_deref() == Some(&row.path) {
            self.expr_editor(ui, layer, row, &mut edit);
        }
        if let Some(e) = edit {
            let (id, path) = (layer.id.clone(), row.path.clone());
            self.edit_layer(&label, e.coalesce(), &id, |l| apply_edit(l, &path, t, &e));
        }
    }

    fn expr_editor(&mut self, ui: &mut egui::Ui, layer: &Layer, row: &Row, edit: &mut Option<PropEdit>) {
        let shared = self.shared.clone();
        ui.add(
            egui::TextEdit::multiline(&mut self.expr_buf)
                .code_editor()
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .hint_text("例: wiggle(3, 20)　／　loopOut()　／　thisComp.layer(\"名前\").transform.position"),
        );
        ui.horizontal(|ui| {
            if ui.button("適用").clicked() {
                *edit = Some(PropEdit::Expr((!self.expr_buf.trim().is_empty()).then(|| self.expr_buf.clone())));
            }
            if row.expr.is_some() && ui.button("削除").clicked() {
                *edit = Some(PropEdit::Expr(None));
                self.expr_buf.clear();
            }
            // ピックウィップの代わり: 参照したいプロパティをメニューから選んで挿入する
            ui.menu_button("参照を挿入", |ui| {
                let Some(comp) = shared.comp(&self.comp_id) else { return };
                for other in &comp.layers {
                    ui.menu_button(&other.name, |ui| {
                        for (label, code) in [("位置", "transform.position"), ("スケール", "transform.scale"), ("回転", "transform.rotation"), ("不透明度", "transform.opacity")] {
                            if ui.button(label).clicked() {
                                self.expr_buf.push_str(&format!("thisComp.layer(\"{}\").{code}", other.name));
                                ui.close_menu();
                            }
                        }
                        for fx in &other.effects {
                            if let Some(def) = effect_def(&fx.kind) {
                                for p in def.params.iter().filter(|p| matches!(p.kind, Pd::F(..) | Pd::P(..) | Pd::C(..))) {
                                    if ui.button(format!("{} › {}", fx.name, p.label)).clicked() {
                                        self.expr_buf.push_str(&format!("thisComp.layer(\"{}\").effect(\"{}\")(\"{}\")", other.name, fx.name, p.label));
                                        ui.close_menu();
                                    }
                                }
                            }
                        }
                    });
                }
            });
            ui.menu_button("定型", |ui| {
                for (label, code) in [
                    ("揺らす wiggle", "wiggle(3, 20)"),
                    ("繰り返す loopOut", "loopOut(\"cycle\")"),
                    ("行き来する loopOut pingpong", "loopOut(\"pingpong\")"),
                    ("時間で回す", "time * 90"),
                    ("最寄りのマーカーで跳ねる", "var m = thisComp.marker;\nvar d = m.numKeys > 0 ? Math.abs(time - m.nearestKey(time).time) : 1;\nvalue * (1 + 0.3 * Math.exp(-d * 12))"),
                    ("コマ落とし posterizeTime", "posterizeTime(12);\nvalue"),
                ] {
                    if ui.button(label).clicked() {
                        self.expr_buf = code.into();
                        ui.close_menu();
                    }
                }
            });
        });
        for e in self.expr_errors.1.iter().filter(|e| e.layer == layer.name && e.path == row.path) {
            let line = e.line.map(|l| format!("{l} 行目: ")).unwrap_or_default();
            ui.colored_label(theme::ERROR, format!("{line}{}", e.message));
        }
    }

    fn rows_with_prefix(&mut self, ui: &mut egui::Ui, layer: &Layer, rows: &[Row], prefix: &str, t: Time) {
        for row in rows.iter().filter(|r| r.path.starts_with(prefix)) {
            if self.only_animated && !row.animated && row.expr.is_none() {
                continue;
            }
            self.prop_row(ui, layer, row, t);
        }
    }

    pub(crate) fn layer_panel(&mut self, ui: &mut egui::Ui, comp: &Composition, layer: &Layer) {
        let id = layer.id.clone();
        let t = comp.frame_rate.frame_to_time(self.frame);
        let rows = collect_rows(layer, t);
        ui.add_enabled_ui(!layer.locked, |ui| {
            widgets::section(ui, "レイヤー", true, |ui| {
                let mut name = layer.name.clone();
                if ui.add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY)).changed() {
                    self.edit_layer("レイヤー名", true, &id, |l| l.name = name);
                }
                self.kind_panel(ui, layer);
                ui.horizontal(|ui| {
                    ui.label("描画モード");
                    widgets::combo(ui, "blend", 0.0, layer.blend_mode.label_ja(), BlendMode::ALL.len(), |ui, pick| {
                        for mode in BlendMode::ALL {
                            if pick.item(ui, mode == layer.blend_mode, mode.label_ja()) {
                                self.edit_layer("描画モード", false, &id, |l| l.blend_mode = mode);
                            }
                        }
                    });
                });
                let others: Vec<&Layer> = comp.layers.iter().filter(|l| l.id != id).collect();
                let name_of = |lid: &LayerId| comp.layer(lid).map(|l| l.name.clone()).unwrap_or_else(|| format!("（不明: {lid}）"));
                ui.horizontal_wrapped(|ui| {
                    ui.label("親");
                    let current = layer.parent.as_ref().map(&name_of).unwrap_or_else(|| "なし".into());
                    widgets::combo(ui, "parent", 0.0, current, others.len() + 1, |ui, pick| {
                        if pick.item(ui, layer.parent.is_none(), "なし") {
                            self.edit_layer("親", false, &id, |l| l.parent = None);
                        }
                        for o in &others {
                            if pick.item(ui, layer.parent.as_ref() == Some(&o.id), &o.name) {
                                let pid = o.id.clone();
                                self.edit_layer("親", false, &id, |l| l.parent = Some(pid));
                                if !self.project.validate().is_empty() {
                                    self.undo();
                                    self.redo.clear();
                                    self.status = "親子関係が循環するため設定できません".into();
                                }
                            }
                        }
                    });
                    ui.label("マット");
                    let current = layer.track_matte.as_ref().map(|m| name_of(&m.layer)).unwrap_or_else(|| "なし".into());
                    widgets::combo(ui, "matte", 0.0, current, others.len() + 1, |ui, pick| {
                        if pick.item(ui, layer.track_matte.is_none(), "なし") {
                            self.edit_layer("トラックマット", false, &id, |l| l.track_matte = None);
                        }
                        for o in &others {
                            if pick.item(ui, layer.track_matte.as_ref().is_some_and(|m| m.layer == o.id), &o.name) {
                                let mode = layer.track_matte.as_ref().map(|m| m.mode).unwrap_or(MatteMode::Alpha);
                                let mid = o.id.clone();
                                self.edit_layer("トラックマット", false, &id, |l| {
                                    l.track_matte = Some(TrackMatte { layer: mid, mode })
                                });
                            }
                        }
                    });
                    if let Some(m) = &layer.track_matte {
                        const MODES: [(MatteMode, &str); 4] = [
                            (MatteMode::Alpha, "アルファ"),
                            (MatteMode::AlphaInverted, "アルファ反転"),
                            (MatteMode::Luma, "ルミナンス"),
                            (MatteMode::LumaInverted, "ルミナンス反転"),
                        ];
                        let label = MODES.iter().find(|(mode, _)| *mode == m.mode).map(|(_, l)| *l).unwrap_or("");
                        widgets::combo(ui, "matte-mode", 0.0, label, MODES.len(), |ui, pick| {
                            for (mode, label) in MODES {
                                if pick.item(ui, mode == m.mode, label) {
                                    self.edit_layer("マットモード", false, &id, |l| {
                                        if let Some(tm) = &mut l.track_matte {
                                            tm.mode = mode;
                                        }
                                    });
                                }
                            }
                        });
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    let flag = |ui: &mut egui::Ui, app: &mut App, label: &str, value: bool, set: fn(&mut Layer, bool)| {
                        let mut v = value;
                        if ui.checkbox(&mut v, label).changed() {
                            app.edit_layer(label, false, &id, |l| set(l, v));
                        }
                    };
                    flag(ui, self, "3D", layer.three_d, |l, v| l.three_d = v);
                    flag(ui, self, "モーションブラー", layer.motion_blur, |l, v| l.motion_blur = v);
                    flag(ui, self, "シャイ", layer.shy, |l, v| l.shy = v);
                    flag(ui, self, "ガイド", layer.guide, |l, v| l.guide = v);
                    if matches!(layer.kind, LayerKind::Precomp { .. }) {
                        flag(ui, self, "コラップス", layer.collapse, |l, v| l.collapse = v);
                    }
                    if matches!(layer.kind, LayerKind::Footage { .. }) {
                        flag(ui, self, "フレームブレンド", layer.frame_blend, |l, v| l.frame_blend = v);
                    }
                    if layer.three_d {
                        flag(ui, self, "ライトを受ける", layer.accepts_lights, |l, v| l.accepts_lights = v);
                        flag(ui, self, "影を落とす", layer.casts_shadows, |l, v| l.casts_shadows = v);
                    }
                });
                ui.horizontal(|ui| {
                    let fr = comp.frame_rate;
                    let (mut fin, mut fout) = (fr.time_to_frame(layer.in_point), fr.time_to_frame(layer.out_point));
                    ui.label("イン／アウト");
                    let ci = ui.add(egui::DragValue::new(&mut fin)).changed();
                    let co = ui.add(egui::DragValue::new(&mut fout)).changed();
                    if ci || co {
                        let fout = fout.max(fin + 1);
                        self.edit_layer("イン／アウト", true, &id, |l| {
                            l.in_point = fr.frame_to_time(fin);
                            l.out_point = fr.frame_to_time(fout);
                        });
                    }
                    let mut start = fr.time_to_frame(layer.start_time);
                    if ui.add(egui::DragValue::new(&mut start).prefix("開始 ")).on_hover_text("ソースの先頭が来るフレーム").changed()
                    {
                        self.edit_layer("開始位置", true, &id, |l| l.start_time = fr.frame_to_time(start));
                    }
                });
                if matches!(layer.kind, LayerKind::Footage { .. } | LayerKind::Precomp { .. } | LayerKind::Audio { .. }) {
                    ui.horizontal(|ui| {
                        let mut speed = layer.speed * 100.0;
                        if ui
                            .add(egui::DragValue::new(&mut speed).range(1.0..=1000.0).prefix("速度 ").suffix("%"))
                            .on_hover_text("映像と音声の両方に掛かります（音程は変わりません）")
                            .changed()
                        {
                            self.edit_layer("再生速度", true, &id, |l| l.speed = speed / 100.0);
                        }
                        widgets::combo(ui, "hold", 0.0, if layer.hold_frames <= 1 {
                                "コマ打ちなし".to_string()
                            } else {
                                format!("{} コマ打ち", layer.hold_frames)
                            }, 5, |ui, pick| {
                                for n in [1u32, 2, 3, 4, 6] {
                                    if pick.item(ui, 
                                            layer.hold_frames == n,
                                            if n == 1 { "コマ打ちなし".to_string() } else { format!("{n} コマ打ち") },
                                        )
                                    {
                                        self.edit_layer("コマ打ち", false, &id, |l| l.hold_frames = n);
                                    }
                                }
                            });
                        if layer.time_remap.is_none() && ui.button("タイムリマップ").clicked() {
                            self.edit_layer("タイムリマップ", false, &id, |l| apply_edit(l, "time_remap", t, &PropEdit::AddKey));
                        }
                    });
                }
                self.audio_controls(ui, layer);
            });
            widgets::section(ui, "トランスフォーム", true, |ui| {
                let three =
                    layer.three_d || matches!(layer.kind, LayerKind::Camera { .. } | LayerKind::Light { .. } | LayerKind::Model { .. });
                for row in rows.iter().filter(|r| r.path.starts_with("transform.")) {
                    let z_only = matches!(
                        row.path.as_str(),
                        "transform.position_z" | "transform.anchor_z" | "transform.rotation_x" | "transform.rotation_y"
                    );
                    if (z_only && !three) || (self.only_animated && !row.animated && row.expr.is_none()) {
                        continue;
                    }
                    self.prop_row(ui, layer, row, t);
                }
                self.rows_with_prefix(ui, layer, &rows, "time_remap", t);
                self.rows_with_prefix(ui, layer, &rows, "audio.", t);
                self.rows_with_prefix(ui, layer, &rows, "camera.", t);
                self.rows_with_prefix(ui, layer, &rows, "light.", t);
            });
            if matches!(layer.kind, LayerKind::Shape { .. }) {
                widgets::section(ui, "コンテンツ", true, |ui| self.rows_with_prefix(ui, layer, &rows, "shape.", t));
            }
            if rows.iter().any(|r| r.path.starts_with("text.")) {
                widgets::section(ui, "テキストアニメーター", true, |ui| self.rows_with_prefix(ui, layer, &rows, "text.", t));
            }
            self.masks_panel(ui, layer, &rows, t);
            self.effects_panel(ui, comp, layer, &rows, t);
        });
    }

    /// 音のあるレイヤーの設定（ミュート・フェード・ピッチ）。
    pub(crate) fn audio_controls(&mut self, ui: &mut egui::Ui, layer: &Layer) {
        let id = layer.id.clone();
        if znd_audio::layer_footage(&self.shared, layer).is_some() {
            ui.horizontal_wrapped(|ui| {
                let mut a = layer.audio.clone();
                let mut changed = ui.checkbox(&mut a.mute, "ミュート").changed();
                let (mut fi, mut fo) = (a.fade_in.seconds(), a.fade_out.seconds());
                if ui.add(egui::DragValue::new(&mut fi).speed(0.01).range(0.0..=60.0).prefix("フェードイン ").suffix("秒")).changed()
                {
                    a.fade_in = Time::from_seconds(fi);
                    changed = true;
                }
                if ui.add(egui::DragValue::new(&mut fo).speed(0.01).range(0.0..=60.0).prefix("アウト ").suffix("秒")).changed() {
                    a.fade_out = Time::from_seconds(fo);
                    changed = true;
                }
                changed |=
                    ui.add(egui::DragValue::new(&mut a.pitch).speed(0.1).range(-24.0..=24.0).prefix("ピッチ ").suffix(" 半音")).changed();
                if changed {
                    self.edit_layer("オーディオ設定", true, &id, |l| l.audio = a);
                }
            });
        }
    }

    /// レイヤー種別ごとの設定（平面の色、テキスト、シェイプの追加など）。
    pub(crate) fn kind_panel(&mut self, ui: &mut egui::Ui, layer: &Layer) {
        let id = layer.id.clone();
        match &layer.kind {
            LayerKind::Solid { width, height, color } => {
                ui.horizontal(|ui| {
                    ui.label("色");
                    let mut c = *color;
                    if ui.color_edit_button_rgb(&mut c).changed() {
                        let (w, h) = (*width, *height);
                        self.edit_layer("平面の色", true, &id, |l| l.kind = LayerKind::Solid { width: w, height: h, color: c });
                    }
                });
            }
            LayerKind::Text { document, animators, .. } => {
                let mut doc = document.clone();
                let mut changed = ui.add(egui::TextEdit::multiline(&mut doc.text).desired_rows(2).desired_width(f32::INFINITY)).changed();
                ui.horizontal_wrapped(|ui| {
                    let current =
                        if doc.font_family.is_empty() { "（既定のフォント）".to_string() } else { doc.font_family.clone() };
                    // 一覧の各行に、本文をそのフォントで描いた見本を出す
                    let sample = crate::font_preview::sample_of(&doc.text);
                    let previews = &mut self.font_previews;
                    let fonts = font_list();
                    widgets::combo(ui, "font", 150.0, current, fonts.len() + 1, |ui, pick| {
                        let ctx = ui.ctx().clone();
                        for (family, label, keys) in std::iter::once(("", "（既定のフォント）", "（既定のフォント）")).chain(fonts.iter().map(|(f, k)| (f.as_str(), f.as_str(), k.as_str()))) {
                            let selected = doc.font_family == family;
                            let mut row = |p: &egui::Painter, rect: egui::Rect| {
                                let color = if selected { egui::Color32::WHITE } else { theme::TEXT };
                                p.text(rect.left_top() + vec2(6.0, 3.0), Align2::LEFT_TOP, label, FontId::proportional(10.0), theme::TEXT_DIM);
                                // （custom は見えている行でだけ描くので、見本も見えている行の分だけ作られる）
                                let h = 22.0;
                                let ppp = ctx.pixels_per_point();
                                match previews.get(&ctx, family, &sample, h * ppp) {
                                    Some(tex) => {
                                        let size = tex.size_vec2() / ppp;
                                        let r = egui::Rect::from_min_size(pos2(rect.left() + 6.0, rect.top() + 15.0), size);
                                        let uv = egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                                        p.image(tex.id(), r, uv, color);
                                    }
                                    None => {
                                        p.text(pos2(rect.left() + 6.0, rect.top() + 26.0), Align2::LEFT_CENTER, "…", FontId::proportional(11.0), theme::TEXT_DIM);
                                    }
                                }
                            };
                            if pick.custom(ui, selected, keys, 40.0, &mut row) {
                                doc.font_family = family.to_string();
                                changed = true;
                            }
                        }
                    })
                    .on_hover_text(if doc.font_family.is_empty() { "既定のフォント" } else { doc.font_family.as_str() });
                    changed |= ui.add(egui::DragValue::new(&mut doc.size).range(1.0..=2000.0).suffix(" px")).changed();
                    changed |= ui.add(egui::DragValue::new(&mut doc.weight).range(100..=900).speed(100.0).prefix("太さ ")).changed();
                });
                if !znd_text::font_available(&doc.font_family) {
                    ui.colored_label(theme::ERROR, format!("フォント「{}」が見つかりません（代替フォントで表示中）", doc.font_family));
                }
                ui.horizontal_wrapped(|ui| {
                    let mut fill = doc.fill.unwrap_or([1.0, 1.0, 1.0, 0.0]).map(|v| v as f32);
                    ui.label("塗り");
                    if ui.color_edit_button_rgba_unmultiplied(&mut fill).changed() {
                        doc.fill = Some(fill.map(|v| v as f64));
                        changed = true;
                    }
                    let mut has_stroke = doc.stroke.is_some();
                    if ui.checkbox(&mut has_stroke, "線").changed() {
                        doc.stroke = has_stroke.then(|| TextStroke { color: [0.0, 0.0, 0.0, 1.0], width: 4.0 });
                        changed = true;
                    }
                    if let Some(s) = &mut doc.stroke {
                        let mut c = s.color.map(|v| v as f32);
                        if ui.color_edit_button_rgba_unmultiplied(&mut c).changed() {
                            s.color = c.map(|v| v as f64);
                            changed = true;
                        }
                        changed |= ui.add(egui::DragValue::new(&mut s.width).range(0.0..=200.0)).changed();
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    for (j, label) in [(Justify::Left, "左"), (Justify::Center, "中央"), (Justify::Right, "右")] {
                        changed |= ui.selectable_value(&mut doc.justify, j, label).changed();
                    }
                    changed |= ui.checkbox(&mut doc.vertical, "縦書き").changed();
                    changed |= ui.add(egui::DragValue::new(&mut doc.tracking).prefix("字間 ")).changed();
                    let mut boxed = doc.box_size.is_some();
                    if ui.checkbox(&mut boxed, "段落").on_hover_text("幅を決めて自動で折り返します").changed() {
                        doc.box_size = boxed.then_some([800.0, 400.0]);
                        changed = true;
                    }
                    if let Some(b) = &mut doc.box_size {
                        changed |= ui.add(egui::DragValue::new(&mut b[0]).range(10.0..=8000.0).prefix("幅 ")).changed();
                    }
                });
                if changed {
                    self.edit_layer("テキスト", true, &id, |l| {
                        if let LayerKind::Text { document, .. } = &mut l.kind {
                            *document = doc;
                        }
                    });
                }
                // アニメーターは定型を選ぶのではなく、空のものを足して属性を選び、下の「テキストアニメーター」で値とキーを付ける
                ui.horizontal_wrapped(|ui| {
                    if ui.button("＋ アニメーター").on_hover_text("文字ごとに動きを付けるアニメーターを足します。属性を選んでから、値やキーフレームを付けてください").clicked() {
                        let name = format!("アニメーター {}", animators.len() + 1);
                        self.edit_layer("アニメーター追加", false, &id, |l| {
                            if let LayerKind::Text { animators, .. } = &mut l.kind {
                                animators.push(TextAnimator { name, selectors: vec![TextSelector::range()], props: AnimatorProps::default() });
                            }
                        });
                    }
                });
                for (i, a) in animators.iter().enumerate() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(&a.name);
                        ui.menu_button("属性", |ui| {
                            for (key, label) in ANIMATOR_PROPS {
                                let mut on = animator_prop_set(&a.props, key);
                                if ui.checkbox(&mut on, label).changed() {
                                    self.edit_layer("アニメーターの属性", false, &id, |l| {
                                        if let LayerKind::Text { animators, .. } = &mut l.kind {
                                            toggle_animator_prop(&mut animators[i].props, key, on);
                                        }
                                    });
                                }
                            }
                        });
                        if let Some(TextSelector::Range { unit, .. }) = a.selectors.first() {
                            let units = [(SelectorUnit::Characters, "文字ごと"), (SelectorUnit::Words, "単語ごと"), (SelectorUnit::Lines, "行ごと")];
                            let current = units.iter().find(|(u, _)| u == unit).map(|(_, l)| *l).unwrap_or("");
                            widgets::combo(ui, ("anim-unit", i), 0.0, current, units.len(), |ui, pick| {
                                for (u, label) in units {
                                    if pick.item(ui, u == *unit, label) {
                                        self.edit_layer("アニメーターの単位", false, &id, |l| {
                                            if let LayerKind::Text { animators, .. } = &mut l.kind {
                                                if let Some(TextSelector::Range { unit, .. }) = animators[i].selectors.first_mut() {
                                                    *unit = u;
                                                }
                                            }
                                        });
                                    }
                                }
                            });
                        }
                        if widgets::small_icon(ui, Icon::Close, false, "このアニメーターを削除").clicked() {
                            self.edit_layer("アニメーター削除", false, &id, |l| {
                                if let LayerKind::Text { animators, .. } = &mut l.kind {
                                    animators.remove(i);
                                }
                            });
                        }
                    });
                }
            }
            LayerKind::Shape { items } => {
                ui.horizontal_wrapped(|ui| {
                    ui.weak(format!("要素 {} 個", items.len()));
                    ui.menu_button("要素を追加", |ui| {
                        let p = |v: f64| Property::constant(v);
                        let options: Vec<(&str, ShapeItem)> = vec![
                            ("矩形", ShapeItem::rect(200.0, 200.0)),
                            ("楕円", ShapeItem::ellipse(200.0, 200.0)),
                            ("多角形", ShapeItem::polygon(6.0, 120.0)),
                            ("スター", ShapeItem::star(5.0, 140.0, 60.0)),
                            ("塗り", ShapeItem::fill([1.0, 1.0, 1.0, 1.0])),
                            ("線", ShapeItem::stroke([1.0, 1.0, 1.0, 1.0], 6.0)),
                            ("トリムパス", ShapeItem::Trim { start: p(0.0), end: p(100.0), offset: p(0.0) }),
                            (
                                "リピーター",
                                ShapeItem::Repeater {
                                    copies: p(3.0),
                                    offset: p(0.0),
                                    transform: shape::ShapeTransform { position: Property::constant([100.0, 0.0]), ..Default::default() },
                                    end_opacity: p(100.0),
                                },
                            ),
                            ("パスのオフセット", ShapeItem::OffsetPath { amount: p(10.0), join: Default::default() }),
                            ("パンク・膨張", ShapeItem::PuckerBloat { amount: p(30.0) }),
                            ("ジグザグ", ShapeItem::ZigZag { size: p(10.0), ridges: p(5.0), smooth: false }),
                            ("波形", ShapeItem::Wave { amplitude: p(10.0), wavelength: p(60.0), phase: p(0.0) }),
                            ("ウィグルパス", ShapeItem::WigglePath { size: p(10.0), detail: p(20.0), speed: p(2.0), seed: 0 }),
                            ("角丸", ShapeItem::RoundCorners { radius: p(20.0) }),
                            ("ねじれ", ShapeItem::Twist { angle: p(60.0), center: Property::constant([0.0, 0.0]) }),
                            ("パスの結合（中マド）", ShapeItem::Merge { mode: shape::MergeMode::Exclude }),
                        ];
                        for (label, item) in options {
                            if ui.button(label).clicked() {
                                ui.close_menu();
                                // 図形とオペレーターは塗り・線より前（上）に入れる。塗り・線は末尾へ
                                let paint = matches!(item, ShapeItem::Fill { .. } | ShapeItem::Stroke { .. });
                                self.edit_layer("シェイプ要素追加", false, &id, |l| {
                                    if let LayerKind::Shape { items } = &mut l.kind {
                                        let at = if paint {
                                            items.len()
                                        } else {
                                            items
                                                .iter()
                                                .position(|i| matches!(i, ShapeItem::Fill { .. } | ShapeItem::Stroke { .. }))
                                                .unwrap_or(items.len())
                                        };
                                        items.insert(at, item);
                                    }
                                });
                            }
                        }
                    });
                    if !items.is_empty() && ui.button("末尾の要素を削除").clicked() {
                        self.edit_layer("シェイプ要素削除", false, &id, |l| {
                            if let LayerKind::Shape { items } = &mut l.kind {
                                items.pop();
                            }
                        });
                    }
                });
            }
            _ => {}
        }
    }

    fn masks_panel(&mut self, ui: &mut egui::Ui, layer: &Layer, rows: &[Row], t: Time) {
        widgets::section(ui, "マスク", !layer.masks.is_empty(), |ui| self.masks_body(ui, layer, rows, t));
    }

    fn masks_body(&mut self, ui: &mut egui::Ui, layer: &Layer, rows: &[Row], t: Time) {
        let id = layer.id.clone();
        ui.horizontal(|ui| {
            let (w, h) = self
                .shared
                .source_size(&layer.kind)
                .unwrap_or_else(|| self.shared.comp(&self.comp_id).map(|c| (c.width as f64, c.height as f64)).unwrap_or((100.0, 100.0)));
            // 寸法を持たないレイヤー（シェイプ・テキスト）は原点が中心
            let centered = self.shared.source_size(&layer.kind).is_none();
            let (ox, oy) = if centered { (-w / 2.0, -h / 2.0) } else { (0.0, 0.0) };
            if ui.button("矩形を追加").clicked() {
                let m = Mask::new(BezPath::rect(ox + w * 0.25, oy + h * 0.25, w * 0.5, h * 0.5));
                self.edit_layer("マスク追加", false, &id, |l| l.masks.push(m));
            }
            if ui.button("楕円を追加").clicked() {
                let m = Mask::new(BezPath::ellipse([ox + w / 2.0, oy + h / 2.0], [w * 0.3, h * 0.3]));
                self.edit_layer("マスク追加", false, &id, |l| l.masks.push(m));
            }
        });
        const MODES: [(MaskMode, &str); 7] = [
            (MaskMode::None, "なし"),
            (MaskMode::Add, "加算"),
            (MaskMode::Subtract, "減算"),
            (MaskMode::Intersect, "交差"),
            (MaskMode::Lighten, "比較（明）"),
            (MaskMode::Darken, "比較（暗）"),
            (MaskMode::Difference, "差"),
        ];
        for (i, mask) in layer.masks.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(format!("マスク {}", i + 1));
                let label = MODES.iter().find(|(m, _)| *m == mask.mode).map(|(_, l)| *l).unwrap_or("");
                widgets::combo(ui, ("mask-mode", i), 0.0, label, MODES.len(), |ui, pick| {
                    for (mode, label) in MODES {
                        if pick.item(ui, mode == mask.mode, label) {
                            self.edit_layer("マスクモード", false, &id, |l| l.masks[i].mode = mode);
                        }
                    }
                });
                let mut inv = mask.inverted;
                if ui.checkbox(&mut inv, "反転").changed() {
                    self.edit_layer("マスク反転", false, &id, |l| l.masks[i].inverted = inv);
                }
                if widgets::small_icon(ui, Icon::Close, false, "このマスクを削除").clicked() {
                    self.edit_layer("マスク削除", false, &id, |l| {
                        l.masks.remove(i);
                    });
                }
            });
            self.rows_with_prefix(ui, layer, rows, &format!("mask.{i}."), t);
        }
    }

    pub(crate) fn effects_panel(&mut self, ui: &mut egui::Ui, comp: &Composition, layer: &Layer, rows: &[Row], t: Time) {
        widgets::section(ui, "エフェクト", true, |ui| self.effects_body(ui, comp, layer, rows, t));
    }

    pub(crate) fn effects_body(&mut self, ui: &mut egui::Ui, comp: &Composition, layer: &Layer, rows: &[Row], t: Time) {
        let id = layer.id.clone();
        ui.horizontal_wrapped(|ui| {
            ui.menu_button("追加", |ui| {
                // 組み込みと、読み込み済みのプラグイン（znd / OpenFX）のエフェクト
                let effects = all_effects();
                let mut categories: Vec<&str> = Vec::new();
                for d in &effects {
                    if !categories.contains(&d.category) {
                        categories.push(d.category);
                    }
                }
                for cat in categories {
                    ui.menu_button(cat, |ui| {
                        for def in effects.iter().filter(|d| d.category == cat) {
                            if ui.button(def.label).clicked() {
                                ui.close_menu();
                                let (w, h) = self.shared.source_size(&layer.kind).unwrap_or((comp.width as f64, comp.height as f64));
                                if let Some(fx) = EffectInstance::new(def.kind, w, h) {
                                    self.edit_layer("エフェクト追加", false, &id, |l| l.effects.push(fx));
                                }
                            }
                        }
                    });
                }
            });
            if !layer.effects.is_empty()
                && ui
                    .button("プリセット保存…")
                    .on_hover_text("このレイヤーのエフェクト（キーフレーム・エクスプレッション込み）をファイルに保存します")
                    .clicked()
            {
                if let Some(path) = rfd::FileDialog::new().set_file_name("preset.zndfx.json").save_file() {
                    let preset = effect::EffectPreset { name: layer.name.clone(), effects: layer.effects.clone() };
                    self.status = match serde_json::to_string_pretty(&preset)
                        .map_err(|e| e.to_string())
                        .and_then(|s| std::fs::write(&path, s).map_err(|e| e.to_string()))
                    {
                        Ok(()) => format!("プリセットを {} に保存しました", path.display()),
                        Err(e) => format!("保存できません: {e}"),
                    };
                }
            }
            if ui.button("プリセット読み込み…").clicked() {
                if let Some(path) = rfd::FileDialog::new().add_filter("エフェクトプリセット", &["json"]).pick_file() {
                    match std::fs::read_to_string(&path)
                        .map_err(|e| e.to_string())
                        .and_then(|s| serde_json::from_str::<effect::EffectPreset>(&s).map_err(|e| e.to_string()))
                    {
                        Ok(preset) => self.edit_layer("プリセット適用", false, &id, |l| l.effects.extend(preset.effects)),
                        Err(e) => self.status = format!("プリセットを読み込めません: {e}"),
                    }
                }
            }
        });
        for (i, fx) in layer.effects.iter().enumerate() {
            let Some(def) = effect_def(&fx.kind) else {
                // 定義が無い = 入っていないプラグインのエフェクト。消さずに残し、外すことだけできるようにする
                ui.horizontal(|ui| {
                    ui.colored_label(theme::ERROR, format!("{}（プラグイン「{}」が入っていません）", fx.name, fx.kind));
                    if widgets::small_icon(ui, Icon::Close, false, "このエフェクトを外す").clicked() {
                        self.edit_layer("エフェクト削除", false, &id, |l| {
                            l.effects.remove(i);
                        });
                    }
                });
                continue;
            };
            ui.horizontal(|ui| {
                let mut on = fx.enabled;
                if ui.checkbox(&mut on, &fx.name).changed() {
                    self.edit_layer("エフェクト有効", false, &id, |l| l.effects[i].enabled = on);
                }
                if i > 0 && widgets::small_icon(ui, Icon::Up, false, "1 つ上へ").clicked() {
                    self.edit_layer("エフェクト順", false, &id, |l| l.effects.swap(i, i - 1));
                }
                if widgets::small_icon(ui, Icon::Close, false, "このエフェクトを削除").clicked() {
                    self.edit_layer("エフェクト削除", false, &id, |l| {
                        l.effects.remove(i);
                    });
                }
            });
            // キーフレームを持てないパラメータ
            for p in def.params {
                let name = p.name.to_string();
                match (&p.kind, fx.params.get(p.name)) {
                    (Pd::B(_), Some(ParamValue::Bool(v))) => {
                        let mut b = *v;
                        if ui.checkbox(&mut b, p.label).changed() {
                            self.edit_layer(p.label, false, &id, |l| {
                                l.effects[i].params.insert(name, ParamValue::Bool(b));
                            });
                        }
                    }
                    (Pd::I(_, labels), Some(ParamValue::Choice(v))) => {
                        ui.horizontal(|ui| {
                            ui.label(p.label);
                            widgets::combo(ui, ("choice", i, p.name), 0.0, labels.get(*v as usize).copied().unwrap_or(""), labels.len(), |ui, pick| {
                                    for (k, label) in labels.iter().enumerate() {
                                        if pick.item(ui, *v as usize == k, *label) {
                                            let name = name.clone();
                                            self.edit_layer(p.label, false, &id, |l| {
                                                l.effects[i].params.insert(name, ParamValue::Choice(k as u32));
                                            });
                                        }
                                    }
                                });
                        });
                    }
                    (Pd::L, Some(ParamValue::Layer(v))) => {
                        ui.horizontal(|ui| {
                            ui.label(p.label);
                            let current = v.as_ref().and_then(|r| comp.layer(r)).map(|l| l.name.clone()).unwrap_or_else(|| "なし".into());
                            widgets::combo(ui, ("layer", i, p.name), 0.0, current, comp.layers.len(), |ui, pick| {
                                for other in &comp.layers {
                                    if pick.item(ui, v.as_ref() == Some(&other.id), &other.name) {
                                        let (name, oid) = (name.clone(), other.id.clone());
                                        self.edit_layer(p.label, false, &id, |l| {
                                            l.effects[i].params.insert(name, ParamValue::Layer(Some(oid)));
                                        });
                                    }
                                }
                            });
                        });
                    }
                    (Pd::S, Some(ParamValue::Text(v))) => {
                        ui.horizontal(|ui| {
                            ui.label(p.label);
                            ui.weak(if v.is_empty() { "（未選択）" } else { v });
                            if ui.small_button("選択…").clicked() {
                                if let Some(path) = rfd::FileDialog::new().pick_file() {
                                    self.edit_layer(p.label, false, &id, |l| {
                                        l.effects[i].params.insert(name, ParamValue::Text(path.display().to_string()));
                                    });
                                }
                            }
                        });
                    }
                    (Pd::Curve(_), Some(ParamValue::Curve(pts))) => {
                        ui.weak(format!("{}: {} 点（プロジェクトファイルまたはスクリプトで編集）", p.label, pts.len()));
                    }
                    _ => {}
                }
            }
            self.rows_with_prefix(ui, layer, rows, &format!("fx.{i}."), t);
        }
    }

    /// 提出前チェックをバックグラウンドで走らせる。
    pub(crate) fn start_check(&mut self, ctx: &egui::Context) {
        let (project, comp_id, footage, clips, dir) = (
            self.shared.clone(),
            self.comp_id.clone(),
            self.engine.footage.clone(),
            self.engine.clips.clone(),
            base_dir(self.path.as_deref()),
        );
        self.check_task = Some(Task::spawn("提出前チェック", ctx, move |report| {
            let comp = project.comp(&comp_id).ok_or("コンポがありません")?;
            let mut out = crate::CheckResult::default();
            let mut section = |title: &str, lines: Vec<String>| {
                if lines.is_empty() {
                    out.lines.push((true, format!("{title}: 問題なし")));
                } else {
                    lines.into_iter().for_each(|l| out.lines.push((false, format!("{title}: {l}"))));
                }
            };
            section("構造", project.validate());
            section("権利", rights::export_warnings(&project, &comp_id));
            section(
                "欠落フッテージ",
                znd_media::manage::missing_footage(&project, &dir)
                    .iter()
                    .filter_map(|id| project.footage_item(id))
                    .map(|f| f.path.clone())
                    .collect(),
            );
            section("欠落フォント", znd_media::manage::missing_fonts(&project));
            let exprs = znd_script::ExprEngine::new();
            section("エクスプレッション", exprs.check(&project, &comp_id, Time::ZERO).iter().map(|e| e.to_string()).collect());
            section(
                "解像度",
                if (comp.width, comp.height) == (1920, 1080) {
                    vec![]
                } else {
                    vec![format!("{}x{}（上映提出は 1920x1080 が基本）", comp.width, comp.height)]
                },
            );
            let ectx = znd_media::ExportContext {
                project: &project,
                footage: &*footage,
                clips: Some(&*clips),
                resolver: Some(&exprs),
                base_dir: Some(dir.clone()),
                accel: crate::gpu(),
                composite: crate::gpu_composite(),
            };
            let flashes = znd_media::review::scan_flashes(&ectx, &comp_id, |d, t| report(d, t)).map_err(|e| e.to_string())?;
            let fr = comp.frame_rate;
            section(
                "光過敏性",
                flashes
                    .iter()
                    .map(|w| {
                        format!(
                            "{:.2}〜{:.2} 秒 {}",
                            fr.frame_to_time(w.start_frame).seconds(),
                            fr.frame_to_time(w.end_frame).seconds(),
                            w.message()
                        )
                    })
                    .collect(),
            );
            let mixer = Mixer::new(&project, &*clips);
            let mixed = mixer.mix(&comp_id, 0, mixer.comp_frames(comp))?;
            if mixed.iter().any(|v| *v != 0.0) {
                let l = znd_audio::loudness::analyze(&mixed);
                let lufs = l.integrated_lufs.map(|v| format!("{v:.1} LUFS")).unwrap_or_else(|| "計測不能".into());
                out.lines.push((true, format!("ラウドネス {lufs} / トゥルーピーク {:.1} dBTP", l.true_peak_db)));
                let warnings = l.warnings(-16.0, -1.0);
                if warnings.is_empty() {
                    out.lines.push((true, "音量: 問題なし".into()));
                } else {
                    warnings.into_iter().for_each(|w| out.lines.push((false, format!("音量: {w}"))));
                }
            }
            out.flashes = flashes;
            Ok(out)
        }));
    }
}
