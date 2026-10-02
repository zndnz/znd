//! 音声再生。コンポの音をあらかじめミックスしておき、出力デバイスのコールバックから読む。
//! 再生中の時刻はこの再生位置（サンプル数）から決まるので、映像が音に合わせて進む。

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

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
}

pub struct Player {
    _stream: cpal::Stream,
    shared: Arc<Shared>,
}

impl Player {
    /// 既定の出力デバイスを開く。デバイスが無い・形式が合わないときは `None`（無音で動く）。
    pub fn new() -> Option<Player> {
        let device = cpal::default_host().default_output_device()?;
        let config = device.default_output_config().ok()?;
        if config.sample_format() != cpal::SampleFormat::F32 {
            return None;
        }
        let channels = config.channels() as usize;
        // デバイスが 48kHz でないときは、読み進める速さを変えて合わせる（直線補間）
        let step = 48_000.0 / config.sample_rate().0 as f64;
        let shared: Arc<Shared> = Arc::default();
        shared.stop_at.store(-1, Ordering::Relaxed);
        let s = shared.clone();
        let mut frac = 0.0f64;
        let stream = device
            .build_output_stream(
                &config.into(),
                move |data: &mut [f32], _| {
                    data.fill(0.0);
                    if !s.playing.load(Ordering::Relaxed) {
                        return;
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
        Some(Player { _stream: stream, shared })
    }

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

    /// 現在の再生位置（48kHz のサンプルフレーム）。
    pub fn position(&self) -> i64 {
        self.shared.pos.load(Ordering::Relaxed)
    }
}
