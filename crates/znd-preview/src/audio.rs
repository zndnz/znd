//! 音声再生。コンポの音をあらかじめミックスしておき、出力デバイスのコールバックから読む。
//! 再生中の時刻はこの再生位置（サンプル数）から決まるので、映像が音に合わせて進む。
//!
//! デバイスに渡したサンプルが実際にスピーカーから鳴るのは、出力の遅延（内蔵スピーカーで数十 ms、
//! Bluetooth では 150 ms 以上）のあと。渡した位置をそのまま時計にすると映像が音より先に進むので、
//! コールバックごとに「渡し始めた位置・その時刻・鳴るまでの遅延」を記録し、いま鳴っている位置を
//! [`heard_position`] で求める。さらに利用者が補正（[`Player::set_offset_ms`]）を足せる。

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use znd_core::{CompId, Composition, EffectInstance, LayerKind, Project};

/// コンポの音に関わる設定だけから作る署名。これが変わったときだけミックスし直せばよい。
pub fn signature(project: &Project, comp: &CompId) -> u64 {
    let Some(comp) = project.comp(comp) else { return 0 };
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut feed = |s: &str| s.bytes().for_each(|b| h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3));
    feed(&comp.id.0);
    fn walk(p: &Project, c: &Composition, feed: &mut dyn FnMut(&str), depth: usize) {
        for l in &c.layers {
            let nested = match &l.kind {
                LayerKind::Precomp { comp } if depth < 16 => p.comp(comp),
                _ => None,
            };
            // 音声エフェクト（プラグイン）のパラメータが変わったときもミックスし直す
            let audio_fx: Vec<&EffectInstance> =
                l.effects.iter().filter(|e| znd_core::effect::audio_processor(&e.kind).is_some()).collect();
            if znd_audio::layer_footage(p, l).is_some() || nested.is_some() || !audio_fx.is_empty() {
                feed(
                    &serde_json::to_string(&(&l.kind, l.in_point, l.out_point, l.start_time, l.speed, &l.audio, &l.time_remap, l.solo))
                        .unwrap_or_default(),
                );
                feed(&serde_json::to_string(&audio_fx).unwrap_or_default());
            }
            if let Some(n) = nested {
                walk(p, n, feed, depth + 1);
            }
        }
    }
    walk(project, comp, &mut feed, 0);
    h
}

#[derive(Default)]
struct Shared {
    /// 48kHz ステレオのミックス済み音声。
    buffer: Mutex<Arc<Vec<f32>>>,
    /// 再生位置（48kHz のサンプルフレーム）。
    pos: AtomicI64,
    playing: AtomicBool,
    /// この位置まで鳴らしたら止める（スクラブ時の短い試聴）。負なら無効。
    stop_at: AtomicI64,
    /// ループ範囲 [開始, 終了)。終了が 0 以下なら無効。
    loop_start: AtomicI64,
    loop_end: AtomicI64,
    /// 最後のコールバックの記録。
    clock: Mutex<Option<ClockSample>>,
    /// 利用者の補正（マイクロ秒）。正の値で映像を遅らせる（音が遅れて聞こえる環境向け）。
    offset_us: AtomicI64,
}

/// コールバック 1 回ぶんの時計の記録。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClockSample {
    /// このコールバックで渡し始めたサンプル位置（48kHz）。
    pub pos: i64,
    /// コールバックが呼ばれた時刻。
    pub at: Instant,
    /// 渡したサンプルが鳴るまでの遅延（デバイスの報告）。
    pub latency: Duration,
}

/// 時刻 `now` にスピーカーから鳴っている位置（48kHz のサンプル）。
/// `offset` は利用者の補正（正で映像を遅らせる）。`start` は再生を始めた位置で、鳴り始める前はそこに留める。
/// `loop_range` があれば、その範囲で折り返す。
pub fn heard_position(c: &ClockSample, now: Instant, offset: Duration, start: i64, loop_range: Option<(i64, i64)>) -> i64 {
    let since = now.saturating_duration_since(c.at).as_secs_f64();
    let behind = c.latency.as_secs_f64() + offset.as_secs_f64();
    let mut pos = c.pos + ((since - behind) * 48_000.0).round() as i64;
    if let Some((ls, le)) = loop_range.filter(|(a, b)| b > a) {
        if pos >= le {
            pos = ls + (pos - ls).rem_euclid(le - ls);
        } else if pos < ls && c.pos >= ls {
            // 折り返した直後: まだ折り返す前の末尾が鳴っている
            pos = le - (ls - pos);
        }
    }
    pos.max(start.min(c.pos))
}

/// 出力デバイスへの再生。デバイスのストリーム（スレッドをまたげない）は専用のスレッドが持ち続けるので、
/// `Player` 自体はどのスレッドからでも使える。
pub struct Player {
    shared: Arc<Shared>,
    /// 再生を始めた位置。
    start: AtomicI64,
}

impl Player {
    /// 既定の出力デバイスを開く。デバイスが無い・形式が合わないときは `None`（無音で動く）。
    pub fn new() -> Option<Player> {
        let shared: Arc<Shared> = Arc::default();
        shared.stop_at.store(-1, Ordering::Relaxed);
        let (tx, rx) = std::sync::mpsc::channel();
        let s = shared.clone();
        std::thread::Builder::new()
            .name("znd-audio".into())
            .spawn(move || match open_stream(s) {
                Some(stream) => {
                    let _ = tx.send(true);
                    // ストリームはこのスレッドが持っている間だけ鳴る
                    let _stream = stream;
                    loop {
                        std::thread::park();
                    }
                }
                None => {
                    let _ = tx.send(false);
                }
            })
            .ok()?;
        rx.recv().ok().filter(|ok| *ok)?;
        Some(Player { shared, start: AtomicI64::new(0) })
    }
}

/// 既定の出力デバイスのストリームを開いて鳴らし始める。
fn open_stream(shared: Arc<Shared>) -> Option<cpal::Stream> {
    let device = cpal::default_host().default_output_device()?;
    let config = device.default_output_config().ok()?;
    if config.sample_format() != cpal::SampleFormat::F32 {
        return None;
    }
    let channels = config.channels() as usize;
    // デバイスが 48kHz でないときは、読み進める速さを変えて合わせる（直線補間）
    let step = 48_000.0 / config.sample_rate().0 as f64;
    let s = shared;
    let mut frac = 0.0f64;
    let stream = device
        .build_output_stream(
            &config.into(),
            move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
                data.fill(0.0);
                if !s.playing.load(Ordering::Relaxed) {
                    return;
                }
                let ts = info.timestamp();
                let latency = ts.playback.duration_since(&ts.callback).unwrap_or_default();
                // 画面のスレッドを待たせない（取れなければこの回の記録は飛ばす）
                if let Ok(mut c) = s.clock.try_lock() {
                    *c = Some(ClockSample { pos: s.pos.load(Ordering::Relaxed), at: Instant::now(), latency });
                }
                let buf = s.buffer.lock().map(|b| b.clone()).unwrap_or_default();
                let frames = (buf.len() / 2) as i64;
                let mut pos = s.pos.load(Ordering::Relaxed);
                let stop = s.stop_at.load(Ordering::Relaxed);
                let (ls, le) = (s.loop_start.load(Ordering::Relaxed), s.loop_end.load(Ordering::Relaxed));
                for out in data.chunks_mut(channels) {
                    if le > ls && pos >= le {
                        pos = ls;
                    }
                    if pos >= frames || (stop >= 0 && pos >= stop) {
                        s.playing.store(false, Ordering::Relaxed);
                        s.stop_at.store(-1, Ordering::Relaxed);
                        break;
                    }
                    if pos >= 0 {
                        let i = pos as usize;
                        let next = ((pos + 1).min(frames - 1)) as usize;
                        let f = frac as f32;
                        let l = buf[i * 2] + (buf[next * 2] - buf[i * 2]) * f;
                        let r = buf[i * 2 + 1] + (buf[next * 2 + 1] - buf[i * 2 + 1]) * f;
                        match out.len() {
                            1 => out[0] = (l + r) * 0.5,
                            _ => {
                                out[0] = l;
                                out[1] = r;
                            }
                        }
                    }
                    frac += step;
                    let whole = frac.floor();
                    pos += whole as i64;
                    frac -= whole;
                }
                s.pos.store(pos, Ordering::Relaxed);
            },
            |e| eprintln!("音声出力エラー: {e}"),
            None,
        )
        .ok()?;
    stream.play().ok()?;
    Some(stream)
}

impl Player {
    /// 再生用の音声（48kHz ステレオを交互に並べたもの）を差し替える。再生中でもよい。
    pub fn set_buffer(&self, buffer: Arc<Vec<f32>>) {
        if let Ok(mut b) = self.shared.buffer.lock() {
            *b = buffer;
        }
    }

    pub fn has_audio(&self) -> bool {
        self.shared.buffer.lock().map(|b| b.iter().any(|v| *v != 0.0)).unwrap_or(false)
    }

    pub fn play_from(&self, sample: i64, loop_range: Option<(i64, i64)>) {
        let (ls, le) = loop_range.unwrap_or((0, 0));
        if let Ok(mut c) = self.shared.clock.lock() {
            *c = None;
        }
        self.start.store(sample, Ordering::Relaxed);
        self.shared.loop_start.store(ls, Ordering::Relaxed);
        self.shared.loop_end.store(le, Ordering::Relaxed);
        self.shared.stop_at.store(-1, Ordering::Relaxed);
        self.shared.pos.store(sample, Ordering::Relaxed);
        self.shared.playing.store(true, Ordering::Relaxed);
    }

    /// `sample` から `len` サンプルだけ鳴らす（スクラブ）。
    pub fn scrub(&self, sample: i64, len: i64) {
        self.shared.loop_end.store(0, Ordering::Relaxed);
        self.shared.pos.store(sample, Ordering::Relaxed);
        self.shared.stop_at.store(sample + len, Ordering::Relaxed);
        self.shared.playing.store(true, Ordering::Relaxed);
    }

    pub fn stop(&self) {
        self.shared.playing.store(false, Ordering::Relaxed);
    }

    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed)
    }

    /// いまスピーカーから鳴っている位置（48kHz のサンプルフレーム）。出力の遅延と利用者の補正を差し引いてある。
    pub fn position(&self) -> i64 {
        let offset_us = self.shared.offset_us.load(Ordering::Relaxed);
        let clock = self.shared.clock.lock().ok().and_then(|c| *c);
        let start = self.start.load(Ordering::Relaxed);
        let (ls, le) = (self.shared.loop_start.load(Ordering::Relaxed), self.shared.loop_end.load(Ordering::Relaxed));
        match clock {
            Some(c) => {
                let behind = Duration::from_micros(offset_us.max(0) as u64);
                let ahead = (-offset_us).max(0) * 48 / 1000;
                heard_position(&c, Instant::now(), behind, start, Some((ls, le))) + ahead
            }
            // まだコールバックが来ていない
            None => start,
        }
    }

    /// 映像と音のずれの補正（ミリ秒）。正の値で映像を遅らせる。
    pub fn set_offset_ms(&self, ms: f64) {
        self.shared.offset_us.store((ms * 1000.0).round() as i64, Ordering::Relaxed);
    }

    /// デバイスが報告している出力の遅延。
    pub fn latency(&self) -> Option<Duration> {
        self.shared.clock.lock().ok().and_then(|c| c.map(|c| c.latency))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
    fn player_is_send_sync() {
        fn f<T: Send + Sync>() {}
        f::<Player>();
    }

    fn at(c: &ClockSample, ms: u64) -> Instant {
        c.at + Duration::from_millis(ms)
    }

    #[test]
    fn heard_position_subtracts_device_latency() {
        let c = ClockSample { pos: 48_000, at: Instant::now(), latency: Duration::from_millis(40) };
        // コールバック直後は、40ms 前に渡したもの（= 1920 サンプル手前）が鳴っている
        assert_eq!(heard_position(&c, at(&c, 0), Duration::ZERO, 0, None), 48_000 - 1920);
        // 40ms 経つと、渡し始めた位置がちょうど鳴る
        assert_eq!(heard_position(&c, at(&c, 40), Duration::ZERO, 0, None), 48_000);
        // 次のコールバックが遅れても、時間に沿って進む
        assert_eq!(heard_position(&c, at(&c, 140), Duration::ZERO, 0, None), 48_000 + 4800);
        // 利用者の補正（Bluetooth などで +150ms）
        assert_eq!(heard_position(&c, at(&c, 40), Duration::from_millis(150), 0, None), 48_000 - 7200);
    }

    #[test]
    fn stays_at_start_until_sound_begins() {
        let c = ClockSample { pos: 96_000, at: Instant::now(), latency: Duration::from_millis(100) };
        assert_eq!(heard_position(&c, at(&c, 0), Duration::ZERO, 96_000, None), 96_000);
        assert_eq!(heard_position(&c, at(&c, 150), Duration::ZERO, 96_000, None), 96_000 + 2400);
    }

    #[test]
    fn wraps_inside_loop_range() {
        let lp = Some((0, 48_000));
        // 折り返した直後のコールバック: 鳴っているのは折り返す前の末尾
        let c = ClockSample { pos: 480, at: Instant::now(), latency: Duration::from_millis(20) };
        assert_eq!(heard_position(&c, at(&c, 0), Duration::ZERO, 0, lp), 48_000 - 480);
        // 末尾を越えたら先頭へ
        let c = ClockSample { pos: 47_000, at: Instant::now(), latency: Duration::ZERO };
        assert_eq!(heard_position(&c, at(&c, 100), Duration::ZERO, 0, lp), 47_000 + 4800 - 48_000);
    }

    /// 1 秒ぶんのコールバック（10ms ごと・遅延 30ms）を模擬して、映像のフレームが音と ±0.5 フレーム以内に収まるか。
    #[test]
    fn simulated_playback_stays_within_half_a_frame() {
        let fps = 30.0;
        let latency = Duration::from_millis(30);
        let t0 = Instant::now();
        let mut worst = 0.0f64;
        for cb in 0..100u64 {
            let c = ClockSample { pos: cb as i64 * 480, at: t0 + Duration::from_millis(cb * 10), latency };
            // コールバックとコールバックの間に、画面が何度か描かれる
            for draw in [0u64, 3, 7] {
                let now = c.at + Duration::from_millis(draw);
                // 鳴っている位置の正解: 時刻 - 遅延
                let truth = (now.duration_since(t0).as_secs_f64() - latency.as_secs_f64()).max(0.0) * 48_000.0;
                let got = heard_position(&c, now, Duration::ZERO, 0, None) as f64;
                worst = worst.max(((got - truth) / 48_000.0 * fps).abs());
            }
        }
        assert!(worst < 0.5, "最大 {worst:.3} フレームずれた");
    }
}
