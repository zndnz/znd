//! 「ビートに合わせる」ウィンドウ（レイヤーの自動配置・パルス）と、映像と音のずれの確認・補正。

use crate::{base_dir, theme, widgets, App};
use eframe::egui;
use znd_core::beatfit::{self, BeatSource, StampMode};
use znd_core::*;

pub(crate) struct BeatUi {
    /// 0 = 毎拍、1 = 2 拍ごと、2 = 小節頭、3 = グリッドの分割ごと、4 = コンポのマーカー、5 = レイヤーのマーカー
    pub source: usize,
    /// ワークエリアの中だけ（無ければコンポ全体）。
    pub work_area_only: bool,
    pub pulse_delta: f64,
    pub pulse_frames: u32,
}

impl Default for BeatUi {
    fn default() -> Self {
        BeatUi { source: 0, work_area_only: true, pulse_delta: 15.0, pulse_frames: 6 }
    }
}

const SOURCES: [&str; 6] = ["毎拍（ビートグリッド）", "2 拍ごと", "小節頭", "グリッドの分割ごと", "コンポのマーカー", "選択レイヤーのマーカー"];

impl App {
    fn beat_source(&self) -> BeatSource {
        let comp = self.shared.comp(&self.comp_id);
        let bar = comp.and_then(|c| c.beat_grid).map(|g| g.beats_per_bar.max(1)).unwrap_or(4);
        match self.beat_ui.source {
            0 => BeatSource::Grid { every: 1 },
            1 => BeatSource::Grid { every: 2 },
            2 => BeatSource::Grid { every: bar },
            3 => BeatSource::Grid { every: 0 },
            4 => BeatSource::CompMarkers,
            _ => BeatSource::LayerMarkers(self.selected.clone().unwrap_or_else(|| LayerId::new(""))),
        }
    }

    /// 対象の範囲（ワークエリア、または全体）とビートの時刻。
    fn beat_plan(&self) -> Option<((Time, Time), Vec<Time>)> {
        let comp = self.shared.comp(&self.comp_id)?;
        let fr = comp.frame_rate;
        let range = match (self.beat_ui.work_area_only, self.work_area) {
            (true, Some((a, b))) => (fr.frame_to_time(a), fr.frame_to_time(b)),
            _ => (Time::ZERO, comp.duration),
        };
        Some((range, beatfit::beat_times(comp, &self.beat_source(), range)))
    }

    pub(crate) fn beatfit_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show.beatfit;
        egui::Window::new("ビートに合わせる").open(&mut open).default_width(420.0).show(ctx, |ui| self.beatfit_ui(ui));
        self.show.beatfit = open;
    }

    fn beatfit_ui(&mut self, ui: &mut egui::Ui) {
        let shared = self.shared.clone();
        let Some(comp) = shared.comp(&self.comp_id) else { return };
        let layer = self.selected.as_ref().and_then(|id| comp.layer(id)).cloned();
        egui::Grid::new("beat-src").num_columns(2).show(ui, |ui| {
            ui.label("ビート");
            widgets::combo(ui, "beat-source", 0.0, SOURCES[self.beat_ui.source], SOURCES.len(), |ui, pick| {
                for (i, s) in SOURCES.iter().enumerate() {
                    pick.value(ui, &mut self.beat_ui.source, i, *s);
                }
            });
            ui.end_row();
            ui.label("範囲");
            ui.checkbox(&mut self.beat_ui.work_area_only, "ワークエリアの中だけ（B / N で設定）");
            ui.end_row();
        });
        let plan = self.beat_plan();
        let n = plan.as_ref().map(|p| p.1.len()).unwrap_or(0);
        if comp.beat_grid.is_none() && self.beat_ui.source <= 3 {
            ui.colored_label(theme::WARN, "このコンポにはビートグリッドがありません（コンポジション設定で BPM を指定してください）");
        } else {
            ui.weak(format!("対象のビート: {n} 個"));
        }
        ui.separator();

        ui.strong("選択レイヤーを並べる");
        ui.weak(match &layer {
            Some(l) => format!("対象: {}", l.name),
            None => "レイヤーを選択してください".into(),
        });
        let ready = n > 0 && layer.as_ref().is_some_and(|l| !l.locked);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(ready, egui::Button::new("頭出しで並べる（連打）"))
                .on_hover_text("ビートごとに複製し、毎回素材の同じ位置から再生します。音MAD の連打・頭出し")
                .clicked()
            {
                self.stamp(StampMode::Restart);
            }
            if ui
                .add_enabled(ready, egui::Button::new("ビートで切る（続けて再生）"))
                .on_hover_text("素材は流れたまま、ビートの位置で切れ目を入れます（カットの位置だけをビートに合わせる）")
                .clicked()
            {
                self.stamp(StampMode::Continue);
            }
        });
        ui.separator();

        ui.strong("パルス（ビートで跳ねて戻る）");
        let target = self.key_target.clone().filter(|(l, _)| layer.as_ref().is_some_and(|x| &x.id == l)).map(|(_, p)| p);
        let path = target.unwrap_or_else(|| "transform.scale".into());
        ui.weak(format!(
            "対象のプロパティ: {}（タイムラインでプロパティの行をクリックすると変えられます）",
            layer.as_ref().map(|l| crate::panels::path_label(l, &path)).unwrap_or_default()
        ));
        ui.horizontal(|ui| {
            ui.label("変化量");
            ui.add(egui::DragValue::new(&mut self.beat_ui.pulse_delta).speed(0.5));
            ui.label("戻るまで");
            ui.add(egui::DragValue::new(&mut self.beat_ui.pulse_frames).range(1..=120).suffix(" フレーム"));
        });
        if ui.add_enabled(ready, egui::Button::new("キーフレームを打つ")).clicked() {
            if let (Some((_, times)), Some(l)) = (plan, layer) {
                let (delta, len) = (self.beat_ui.pulse_delta, comp.frame_rate.frame_to_time(self.beat_ui.pulse_frames as i64));
                let mut count = 0;
                self.edit_layer("ビートでパルス", false, &l.id, |l| count = beatfit::pulse_on_beats(l, &path, &times, &[delta], len));
                self.status = if count > 0 {
                    format!("{} 個のビートにキーフレームを打ちました", times.len())
                } else {
                    "このプロパティにはキーフレームを打てません".into()
                };
            }
        }
    }

    fn stamp(&mut self, mode: StampMode) {
        let Some(((_, end), times)) = self.beat_plan() else { return };
        let Some(id) = self.selected.clone() else { return };
        let mut made = Vec::new();
        let mut counter = 0;
        let shared = self.shared.clone();
        self.edit_comp("ビートに合わせて配置", false, |c| {
            made = beatfit::stamp_on_beats(c, &id, &times, end, mode, &mut || {
                counter += 1;
                // 編集前のプロジェクトで未使用の ID に、通し番号を足して重ならないようにする
                LayerId::new(format!("{}-{counter}", shared.fresh_id("beat")))
            });
        });
        self.selected_keys.clear();
        self.status = format!("{} 個に並べました（⌘Z で元に戻せます）", made.len());
    }

    /// 映像と音のずれの補正と、確認用のコンポ（オーディオのタブ）。
    pub(crate) fn sync_controls(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.label(egui::RichText::new("映像と音のずれ").strong());
        if let Some(l) = self.player.as_ref().and_then(|p| p.latency()) {
            ui.weak(format!("出力の遅延（自動で補正済み）: {:.0} ms", l.as_secs_f64() * 1000.0));
        }
        ui.horizontal(|ui| {
            ui.label("補正");
            let r = ui
                .add(egui::DragValue::new(&mut self.prefs.av_offset_ms).range(-500.0..=500.0).speed(1.0).suffix(" ms"))
                .on_hover_text("正の値で映像を遅らせます。Bluetooth のイヤホンなど、音が遅れて聞こえるときに使います");
            if r.changed() {
                if let Some(p) = &self.player {
                    p.set_offset_ms(self.prefs.av_offset_ms);
                }
            }
            if r.drag_stopped() || r.lost_focus() {
                if let Err(e) = self.prefs.save() {
                    self.status = format!("設定を保存できません: {e}");
                }
            }
        });
        if ui
            .button("同期テスト用のコンポを作る")
            .on_hover_text("120 BPM のクリック音と、拍ごとに光る画面のコンポを作ります。再生して、光と音が同時に感じられるよう補正してください")
            .clicked()
        {
            self.make_sync_test();
        }
    }

    fn make_sync_test(&mut self) {
        let shared = self.shared.clone();
        let (w, h, fr) = shared.comp(&self.comp_id).map(|c| (c.width, c.height, c.frame_rate)).unwrap_or((1920, 1080, FrameRate::FPS_30));
        let wav = base_dir(self.path.as_deref()).join("znd-sync-test-120bpm.wav");
        let seconds = 8.0;
        if let Err(e) = znd_media::audio::write_wav(&wav, &znd_audio::analysis::click_track(120.0, seconds)) {
            self.status = format!("クリック音を書き出せません: {e}");
            return;
        }
        let id = shared.fresh_id("comp");
        let comp = beatfit::sync_test_comp(&id, w, h, fr, 120.0, seconds);
        self.edit("同期テストのコンポ", false, |p| p.compositions.push(comp));
        self.switch_comp(CompId::new(id));
        self.set_frame(0, false);
        self.import_path(&wav, false);
        self.status = "同期テストのコンポを作りました。再生して、光と音がずれて感じられたら補正を調整してください".into();
    }
}
