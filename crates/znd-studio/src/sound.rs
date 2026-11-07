//! 音声の再生。コンポの音をバックグラウンドでミックスしておき、再生中は出力デバイスの再生位置を
//! 時計にする（画面は [`Sound::clock`] の返すフレームを表示するので、映像が音に合わせて進む）。
//!
//! ミックスは音に関わる設定（[`audio::signature`]）が変わったときだけやり直す。ミックス中に
//! さらに編集されたら、終わってから最新の状態でもう一度ミックスする（途中の状態は飛ばす）。

use std::sync::{Arc, Mutex, MutexGuard};
use znd_audio::{sample_to_time, time_to_sample, Mixer};
use znd_core::{CompId, FrameRate, Project};
use znd_media::FfmpegClips;
use znd_preview::audio::{self, Player};

#[derive(Default)]
struct State {
    /// いまの状態の署名（ミックスが追いつくべきもの）。
    wanted: u64,
    /// プレーヤーに渡してある音声の署名。
    ready: u64,
    /// 渡してある音声に音があるか。
    audible: bool,
    /// まだミックスしていない最新の状態。
    pending: Option<Job>,
    mixing: bool,
    /// 再生中なら、繰り返す範囲（48kHz のサンプル）。
    playing: Option<(i64, i64)>,
}

struct Job {
    sig: u64,
    project: Arc<Project>,
    comp: CompId,
    clips: Arc<FfmpegClips>,
}

pub struct Sound {
    /// 出力デバイスが無いときは `None`（無音で動く）。
    player: Option<Player>,
    state: Mutex<State>,
}

impl Sound {
    pub fn new() -> Arc<Sound> {
        let player = Player::new();
        if player.is_none() {
            eprintln!("音声の出力デバイスを開けません（音なしで再生します）");
        }
        Arc::new(Sound { player, state: Mutex::default() })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 素材の置き場所が変わったなど、ミックスし直す必要があるときに呼ぶ。
    pub fn invalidate(&self) {
        let mut st = self.state();
        st.wanted = 0;
        st.ready = 0;
    }

    /// `comp` の音を、必要ならバックグラウンドでミックスし直す。
    pub fn sync(self: &Arc<Self>, project: &Arc<Project>, comp: &CompId, clips: &Arc<FfmpegClips>) {
        if self.player.is_none() {
            return;
        }
        // 0 は「まだ何も無い」に使うので避ける
        let sig = audio::signature(project, comp).max(1);
        let mut st = self.state();
        if st.wanted == sig {
            return;
        }
        st.wanted = sig;
        st.pending = Some(Job { sig, project: project.clone(), comp: comp.clone(), clips: clips.clone() });
        if st.mixing {
            return;
        }
        st.mixing = true;
        drop(st);
        let me = self.clone();
        let spawned = std::thread::Builder::new().name("znd-mix".into()).spawn(move || me.mix_loop());
        if spawned.is_err() {
            self.state().mixing = false;
        }
    }

    fn mix_loop(&self) {
        loop {
            let Some(job) = self.state().pending.take() else {
                self.state().mixing = false;
                return;
            };
            let mixer = Mixer::new(&job.project, &*job.clips);
            let frames = job.project.comp(&job.comp).map(|c| mixer.comp_frames(c)).unwrap_or(0);
            let result = mixer.mix(&job.comp, 0, frames);
            let mut st = self.state();
            match result {
                Ok(buf) if st.wanted == job.sig => {
                    st.audible = buf.iter().any(|v| *v != 0.0);
                    st.ready = job.sig;
                    if let Some(p) = &self.player {
                        p.set_buffer(Arc::new(buf));
                    }
                }
                Ok(_) => {}
                Err(e) => eprintln!("音声をミックスできません: {e}"),
            }
            // 取りこぼしを防ぐため、pending の確認と mixing の解除は同じロックの中で行う
            if st.pending.is_none() {
                st.mixing = false;
                return;
            }
        }
    }

    /// 再生を始める。`range` は繰り返すフレームの範囲 [開始, 終了)。
    pub fn play(&self, fr: FrameRate, frame: i64, range: (i64, i64)) {
        let range = (time_to_sample(fr.frame_to_time(range.0)), time_to_sample(fr.frame_to_time(range.1)));
        let mut st = self.state();
        st.playing = Some(range);
        if let Some(p) = self.player.as_ref().filter(|_| st.audible && st.ready == st.wanted) {
            p.play_from(time_to_sample(fr.frame_to_time(frame)), Some(range));
        }
    }

    pub fn stop(&self) {
        self.state().playing = None;
        if let Some(p) = &self.player {
            p.stop();
        }
    }

    /// 再生中に、いま鳴っている位置のフレームを返す（音が無ければ `None`。画面は経過時間で進める）。
    /// `frame` は画面が経過時間から求めたフレームで、ミックスが再生の途中で終わったときはそこから鳴らし始める。
    pub fn clock(&self, fr: FrameRate, frame: i64) -> Option<i64> {
        let p = self.player.as_ref()?;
        let st = self.state();
        let range = st.playing?;
        if !st.audible {
            return None;
        }
        if !p.is_playing() && st.ready == st.wanted {
            p.play_from(time_to_sample(fr.frame_to_time(frame)), Some(range));
            return Some(frame);
        }
        p.is_playing().then(|| fr.time_to_frame(sample_to_time(p.position())))
    }
}
