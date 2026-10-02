//! オーディオエンジン: クリップのミキシング、タイムストレッチ、解析、ラウドネス。
//!
//! 内部は 48kHz・ステレオ・f32 に統一する。時刻は flicks なので、コンポ上の位置は
//! 常にサンプル単位で正確に求まる（1 サンプル = 14,700 flicks）。

pub mod analysis;
pub mod dsp;
pub mod loudness;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use znd_core::{CompId, Composition, FootageId, FootageItem, Layer, LayerId, LayerKind, Project, Time, FLICKS_PER_SECOND};
use znd_render::AudioWindow;

pub const SAMPLE_RATE: u32 = 48_000;
const FLICKS_PER_SAMPLE: i64 = FLICKS_PER_SECOND / SAMPLE_RATE as i64;
/// 音量などのパラメータを評価し直す間隔（サンプル）。この間は直線補間する。
const CONTROL_BLOCK: usize = 256;
const MAX_DEPTH: usize = 32;

pub fn time_to_sample(t: Time) -> i64 {
    t.0.div_euclid(FLICKS_PER_SAMPLE)
}

pub fn sample_to_time(s: i64) -> Time {
    Time(s * FLICKS_PER_SAMPLE)
}

/// デコード済みの音声（48kHz ステレオ、インターリーブ）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Clip {
    pub data: Vec<f32>,
}

impl Clip {
    pub fn frames(&self) -> usize {
        self.data.len() / 2
    }

    pub fn mono(&self) -> Vec<f32> {
        self.data.chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5).collect()
    }

    pub fn from_mono(mono: &[f32]) -> Clip {
        Clip { data: mono.iter().flat_map(|v| [*v, *v]).collect() }
    }
}

/// フッテージの音声を供給する（`znd-media` が FFmpeg で実装する）。
pub trait ClipSource: Sync {
    fn clip(&self, item: &FootageItem) -> Result<Arc<Clip>, String>;
}

/// メモリ上のクリップ（テスト用）。
#[derive(Default)]
pub struct MemorySource(pub HashMap<FootageId, Arc<Clip>>);

impl ClipSource for MemorySource {
    fn clip(&self, item: &FootageItem) -> Result<Arc<Clip>, String> {
        self.0.get(&item.id).cloned().ok_or_else(|| format!("{}: 音声がありません", item.name))
    }
}

fn db_to_gain(db: f64) -> f32 {
    10f64.powf(db / 20.0) as f32
}

/// レイヤーが音を持つか。
pub fn layer_footage<'a>(project: &'a Project, layer: &Layer) -> Option<&'a FootageItem> {
    match &layer.kind {
        LayerKind::Footage { footage } | LayerKind::Audio { footage } => project.footage_item(footage).filter(|f| f.audio.is_some()),
        _ => None,
    }
}

pub struct Mixer<'a> {
    pub project: &'a Project,
    pub source: &'a dyn ClipSource,
    /// 再生速度・ピッチを適用済みのクリップ（同じ設定のレイヤーで共有）。
    processed: Mutex<HashMap<(FootageId, u64, u64), Arc<Clip>>>,
}

impl<'a> Mixer<'a> {
    pub fn new(project: &'a Project, source: &'a dyn ClipSource) -> Self {
        Mixer { project, source, processed: Mutex::default() }
    }

    fn clip_for(&self, item: &FootageItem, speed: f64, pitch: f64) -> Result<Arc<Clip>, String> {
        let key = (item.id.clone(), speed.to_bits(), pitch.to_bits());
        if let Some(c) = self.processed.lock().unwrap().get(&key) {
            return Ok(c.clone());
        }
        let raw = self.source.clip(item)?;
        let clip = if (speed - 1.0).abs() < 1e-9 && pitch.abs() < 1e-9 {
            raw
        } else {
            let stretched = dsp::time_stretch(&raw.data, 2, speed.max(0.01));
            Arc::new(Clip { data: dsp::pitch_shift(&stretched, 2, pitch) })
        };
        self.processed.lock().unwrap().insert(key, clip.clone());
        Ok(clip)
    }

    /// コンポの音声を `start` サンプル目から `frames` サンプルぶんミックスする（ステレオ）。
    pub fn mix(&self, comp: &CompId, start: i64, frames: usize) -> Result<Vec<f32>, String> {
        let comp = self.project.comp(comp).ok_or_else(|| format!("コンポ「{comp}」が見つかりません"))?;
        let mut out = vec![0.0f32; frames * 2];
        self.mix_comp(comp, start, &mut out, None, 0)?;
        Ok(out)
    }

    /// 1 レイヤーだけをミックスする（スペクトラム表示・解析用）。
    pub fn mix_layer(&self, comp: &CompId, layer: &LayerId, start: i64, frames: usize) -> Result<Vec<f32>, String> {
        let comp = self.project.comp(comp).ok_or_else(|| format!("コンポ「{comp}」が見つかりません"))?;
        let mut out = vec![0.0f32; frames * 2];
        self.mix_comp(comp, start, &mut out, Some(layer), 0)?;
        Ok(out)
    }

    /// コンポ全尺のサンプル数。
    pub fn comp_frames(&self, comp: &Composition) -> usize {
        time_to_sample(comp.duration).max(0) as usize
    }

    fn mix_comp(&self, comp: &Composition, start: i64, out: &mut [f32], only: Option<&LayerId>, depth: usize) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err("プリコンポのネストが深すぎます（循環参照の可能性）".into());
        }
        let frames = out.len() / 2;
        let any_solo = comp.layers.iter().any(|l| l.solo);
        for layer in &comp.layers {
            if layer.audio.mute || (any_solo && !layer.solo) || only.is_some_and(|id| id != &layer.id) {
                continue;
            }
            // このブロックのうち、レイヤーが鳴っている範囲
            let (lin, lout) = (time_to_sample(layer.in_point), time_to_sample(layer.out_point));
            let (a, b) = ((lin - start).clamp(0, frames as i64) as usize, (lout - start).clamp(0, frames as i64) as usize);
            if b <= a {
                continue;
            }
            let layer_start = time_to_sample(layer.start_time);
            let mut dry = vec![0.0f32; (b - a) * 2];
            match &layer.kind {
                LayerKind::Footage { .. } | LayerKind::Audio { .. } => {
                    let Some(item) = layer_footage(self.project, layer) else { continue };
                    match &layer.time_remap {
                        Some(remap) => {
                            // タイムリマップ: ソース位置を追って読む（音程も一緒に変わる）
                            let clip = self.source.clip(item)?;
                            let pos_at = |abs: i64| remap.eval(sample_to_time(abs)) * SAMPLE_RATE as f64;
                            // 制御点はコンポ上の絶対位置に置く（どこで区切ってミックスしても同じ結果になる）
                            let mut n = a;
                            while n < b {
                                let abs = start + n as i64;
                                let grid = abs.div_euclid(CONTROL_BLOCK as i64) * CONTROL_BLOCK as i64;
                                let end = ((grid + CONTROL_BLOCK as i64 - start) as usize).min(b);
                                let (p0, p1) = (pos_at(grid), pos_at(grid + CONTROL_BLOCK as i64));
                                for i in n..end {
                                    let f = (start + i as i64 - grid) as f64 / CONTROL_BLOCK as f64;
                                    let pos = p0 + (p1 - p0) * f;
                                    dry[(i - a) * 2] = dsp::sample_at(&clip.data, 2, 0, pos);
                                    dry[(i - a) * 2 + 1] = dsp::sample_at(&clip.data, 2, 1, pos);
                                }
                                n = end;
                            }
                        }
                        None => {
                            // 速度・ピッチ適用済みのクリップを、整数サンプル位置でそのまま置く
                            let clip = self.clip_for(item, layer.speed, layer.audio.pitch)?;
                            let total = clip.frames() as i64;
                            for i in a..b {
                                let src = start + i as i64 - layer_start;
                                if src >= 0 && src < total {
                                    dry[(i - a) * 2] = clip.data[src as usize * 2];
                                    dry[(i - a) * 2 + 1] = clip.data[src as usize * 2 + 1];
                                }
                            }
                        }
                    }
                }
                LayerKind::Precomp { comp: nested } => {
                    let Some(nested) = self.project.comp(nested) else { continue };
                    // ネストしたコンポの音は等速で鳴らす（プリコンポの速度変更は映像のみに掛かる）
                    self.mix_comp(nested, start + a as i64 - layer_start, &mut dry, None, depth + 1)?;
                    // ネスト先の尺の外は無音
                    let nested_frames = time_to_sample(nested.duration);
                    for i in a..b {
                        let src = start + i as i64 - layer_start;
                        if src < 0 || src >= nested_frames {
                            dry[(i - a) * 2] = 0.0;
                            dry[(i - a) * 2 + 1] = 0.0;
                        }
                    }
                }
                _ => continue,
            }
            // 音量（キーフレーム可）とフェード
            let (fade_in, fade_out) = (time_to_sample(layer.audio.fade_in).max(0), time_to_sample(layer.audio.fade_out).max(0));
            let gain_at = |abs: i64| db_to_gain(layer.audio.level_db.eval(sample_to_time(abs)));
            let mut n = a;
            while n < b {
                let abs = start + n as i64;
                let grid = abs.div_euclid(CONTROL_BLOCK as i64) * CONTROL_BLOCK as i64;
                let end = ((grid + CONTROL_BLOCK as i64 - start) as usize).min(b);
                let (g0, g1) = (gain_at(grid), gain_at(grid + CONTROL_BLOCK as i64));
                for i in n..end {
                    let pos = start + i as i64;
                    let mut g = g0 + (g1 - g0) * (pos - grid) as f32 / CONTROL_BLOCK as f32;
                    if fade_in > 0 && pos - lin < fade_in {
                        g *= (pos - lin) as f32 / fade_in as f32;
                    }
                    if fade_out > 0 && lout - pos <= fade_out {
                        g *= (lout - pos) as f32 / fade_out as f32;
                    }
                    out[i * 2] += dry[(i - a) * 2] * g;
                    out[i * 2 + 1] += dry[(i - a) * 2 + 1] * g;
                }
                n = end;
            }
        }
        Ok(())
    }
}

impl AudioWindow for Mixer<'_> {
    fn window(&self, _project: &Project, comp: &CompId, layer: &LayerId, center: Time, len: usize) -> Vec<f32> {
        let start = time_to_sample(center) - len as i64 / 2;
        match self.mix_layer(comp, layer, start, len) {
            Ok(stereo) => stereo.chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5).collect(),
            Err(_) => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use znd_core::*;

    const SR: i64 = SAMPLE_RATE as i64;

    fn item(id: &str, seconds: f64) -> FootageItem {
        FootageItem {
            id: FootageId::new(id),
            name: id.into(),
            path: format!("{id}.wav"),
            width: 0,
            height: 0,
            frame_rate: None,
            duration: None,
            rights: Rights::default(),
            audio: Some(AudioInfo { sample_rate: 48000, channels: 2, duration: Time::from_seconds(seconds) }),
            vfr: false,
        }
    }

    fn audio_layer(id: &str, footage: &str, dur: Time) -> Layer {
        Layer::new(id, id, LayerKind::Audio { footage: FootageId::new(footage) }, Transform::centered(0.0, 0.0, 0.0, 0.0), dur)
    }

    /// サンプル番号 n の値が (n+1)/100000 のランプ（位置の検証用）。
    fn ramp(frames: usize) -> Arc<Clip> {
        Arc::new(Clip { data: (0..frames).flat_map(|i| [(i + 1) as f32 / 100_000.0, -((i + 1) as f32) / 100_000.0]).collect() })
    }

    fn setup(layers: Vec<Layer>, clips: Vec<(&str, Arc<Clip>)>) -> (Project, MemorySource) {
        let dur = Time::from_seconds(600.0);
        let mut p = Project::new("a");
        let mut src = MemorySource::default();
        for (id, clip) in clips {
            p.footage.push(item(id, clip.frames() as f64 / SR as f64));
            src.0.insert(FootageId::new(id), clip);
        }
        p.compositions.push(Composition { layers, ..Composition::new("c", "c", 1920, 1080, FrameRate::FPS_23_976, dur) });
        (p, src)
    }

    fn id() -> CompId {
        CompId::new("c")
    }

    #[test]
    fn clip_is_placed_sample_accurately_even_far_into_the_timeline() {
        let dur = Time::from_seconds(600.0);
        let mut l = audio_layer("a", "r", dur);
        // 23.976fps の 7193 フレーム目（約 5 分）から鳴らす
        let start = FrameRate::FPS_23_976.frame_to_time(7193);
        l.start_time = start;
        l.in_point = start;
        let (p, src) = setup(vec![l], vec![("r", ramp(48000))]);
        let m = Mixer::new(&p, &src);
        let s0 = time_to_sample(start);
        assert_eq!(s0, 7193 * 1001 * 2);
        let out = m.mix(&id(), s0 - 2, 6).unwrap();
        // 開始 2 サンプル前は無音、開始サンプルからランプの先頭
        assert_eq!(&out[..4], &[0.0, 0.0, 0.0, 0.0]);
        assert_eq!(out[4], 1.0 / 100_000.0);
        assert_eq!(out[5], -1.0 / 100_000.0);
        assert_eq!(out[10], 4.0 / 100_000.0);
    }

    #[test]
    fn block_boundaries_are_seamless() {
        // どこで区切ってミックスしても、つなげれば 1 回でミックスしたものと完全に一致する（ループ再生の継ぎ目）
        let dur = Time::from_seconds(600.0);
        let mut l = audio_layer("a", "r", dur);
        l.start_time = Time::from_seconds(0.01);
        l.in_point = Time::from_seconds(0.02);
        l.audio.fade_in = Time::from_seconds(0.05);
        l.audio.level_db =
            Property::constant(0.0).with_keys(vec![Keyframe::linear(Time::ZERO, -12.0), Keyframe::linear(Time::from_seconds(0.2), 0.0)]);
        let (p, src) = setup(vec![l], vec![("r", ramp(48000))]);
        let m = Mixer::new(&p, &src);
        let whole = m.mix(&id(), 0, 9600).unwrap();
        let mut pieces = Vec::new();
        for (s, n) in [(0, 1000), (1000, 37), (1037, 5000), (6037, 3563)] {
            pieces.extend(m.mix(&id(), s, n).unwrap());
        }
        assert_eq!(whole, pieces);
    }

    #[test]
    fn level_fades_mute_and_solo() {
        let dur = Time::from_seconds(600.0);
        let ones = Arc::new(Clip { data: vec![1.0; 48000 * 2] });
        let mut a = audio_layer("a", "one", dur);
        a.out_point = Time::from_seconds(1.0);
        a.audio.level_db = Property::constant(-6.0206);
        a.audio.fade_in = Time::from_seconds(0.1);
        a.audio.fade_out = Time::from_seconds(0.1);
        let (mut p, src) = setup(vec![a], vec![("one", ones)]);
        let at = |p: &Project, s: i64| Mixer::new(p, &src).mix(&id(), s, 1).unwrap()[0];
        assert!((at(&p, 24000) - 0.5).abs() < 1e-4);
        assert!((at(&p, 2400) - 0.25).abs() < 1e-4, "フェードイン中間");
        assert!((at(&p, 48000 - 2400) - 0.25).abs() < 1e-4, "フェードアウト中間");
        assert_eq!(at(&p, 48000), 0.0, "アウト点以降は無音");
        p.compositions[0].layers[0].audio.mute = true;
        assert_eq!(at(&p, 24000), 0.0);
        p.compositions[0].layers[0].audio.mute = false;
        // 別のレイヤーがソロなら鳴らない
        let mut other = audio_layer("b", "one", dur);
        other.solo = true;
        other.audio.level_db = Property::constant(-20.0);
        p.compositions[0].layers.push(other);
        assert!((at(&p, 24000) - 0.1).abs() < 1e-4);
    }

    #[test]
    fn layers_sum_and_precomp_audio_is_offset() {
        let dur = Time::from_seconds(600.0);
        let ones = Arc::new(Clip { data: vec![0.25; 48000 * 2] });
        let mut inner = audio_layer("in", "one", dur);
        inner.start_time = Time::from_seconds(0.5);
        inner.in_point = Time::from_seconds(0.5);
        let (mut p, src) = setup(vec![audio_layer("a", "one", dur)], vec![("one", ones)]);
        p.compositions.push(Composition {
            layers: vec![inner],
            ..Composition::new("nested", "nested", 100, 100, FrameRate::FPS_30, Time::from_seconds(10.0))
        });
        let mut pre = Layer::new(
            "pre",
            "pre",
            LayerKind::Precomp { comp: CompId::new("nested") },
            Transform::centered(100.0, 100.0, 100.0, 100.0),
            dur,
        );
        pre.start_time = Time::from_seconds(0.25);
        p.compositions[0].layers.push(pre);
        let m = Mixer::new(&p, &src);
        let at = |s: i64| m.mix(&id(), s, 1).unwrap()[0];
        // 0.5 秒: 直接のレイヤーだけ。ネスト内の音は 0.25 + 0.5 = 0.75 秒から
        assert_eq!(at(24000), 0.25);
        assert_eq!(at(36000 - 1), 0.25);
        assert_eq!(at(36000), 0.5);
        // 直接のクリップが終わる 1.0 秒以降はネスト側だけ（1.75 秒まで）
        assert_eq!(at(60000), 0.25);
        assert_eq!(at(84000), 0.0);
        // 1 レイヤーだけ取り出す
        assert_eq!(m.mix_layer(&id(), &LayerId::new("pre"), 36000, 1).unwrap()[0], 0.25);
        let w = m.window(&p, &id(), &LayerId::new("a"), Time::from_seconds(0.5), 64);
        assert_eq!(w.len(), 64);
        assert!(w.iter().all(|v| *v == 0.25));
    }

    #[test]
    fn speed_pitch_and_time_remap() {
        let dur = Time::from_seconds(600.0);
        let tone: Vec<f32> = (0..96000).map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48000.0).sin() * 0.5).collect();
        let mut fast = audio_layer("a", "tone", dur);
        fast.speed = 2.0;
        let (mut p, src) = setup(vec![fast], vec![("tone", Arc::new(Clip::from_mono(&tone)))]);
        // 2 倍速: 2 秒の素材が 1 秒で終わる（1 秒以降は無音）。音程は変わらない
        let out = Mixer::new(&p, &src).mix(&id(), 0, 60000).unwrap();
        let energy = |s: &[f32]| s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32;
        assert!(energy(&out[..96000]) > 0.05);
        assert_eq!(energy(&out[96002..]), 0.0);
        // タイムリマップで逆再生: コンポ 0 秒 = ソース 1 秒、コンポ 1 秒 = ソース 0 秒
        p.compositions[0].layers[0].speed = 1.0;
        p.compositions[0].layers[0].time_remap = Some(
            Property::constant(0.0).with_keys(vec![Keyframe::linear(Time::ZERO, 1.0), Keyframe::linear(Time::from_seconds(1.0), 0.0)]),
        );
        let rev = Mixer::new(&p, &src).mix(&id(), 0, 48000).unwrap();
        // 位置 n の値はソースの (48000 - n) の値
        for n in [1000usize, 12345, 40000] {
            assert!((rev[n * 2] - tone[48000 - n]).abs() < 1e-3, "n={n}");
        }
    }
}
