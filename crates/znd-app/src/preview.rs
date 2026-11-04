//! RAM プレビュー。バックグラウンドスレッドが現在フレームを最優先で、続いて先のフレームを
//! 並列にレンダリングしてキャッシュに積む。
//!
//! キャッシュのキーはフレームの指紋（`znd_render::fingerprint`）なので、編集しても
//! 影響の無いフレームは作り直さない。環境変数 `ZND_DISK_CACHE` にフォルダを指定すると、
//! 同じキーでディスクにも保存し、次回起動時に再利用する。

use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use znd_audio::Mixer;
use znd_core::{CompId, Project};
use znd_media::{FfmpegClips, FfmpegFootage};
use znd_render::check::{screening_report, ScreeningReport};
use znd_render::fingerprint::Fingerprinter;
use znd_render::{encode_rgba8, RenderOptions, Renderer};
use znd_script::ExprEngine;

/// 他のスレッドがパニックしてもロックを取れるようにする（キャッシュの中身は壊れないので、そのまま使う）。
fn relock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 表示用に 8bit へ変換済みのフレーム。
pub struct Preview {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
    /// 上映プレビュー用の検査結果（白飛び・黒つぶれ・バンディング）。
    pub screening: ScreeningReport,
}

struct Job {
    project: Arc<Project>,
    revision: u64,
    comp: CompId,
    frame: i64,
    downsample: u32,
    hashes: Arc<Fingerprinter>,
    exprs: Arc<ExprEngine>,
}

impl Job {
    fn opts(&self) -> RenderOptions {
        RenderOptions { downsample: self.downsample, ..RenderOptions::PREVIEW }
    }

    fn key(&self, frame: i64) -> u64 {
        match self.project.comp(&self.comp) {
            Some(c) => self.hashes.frame(&self.project, &self.comp, c.frame_rate.frame_to_time(frame), self.opts()),
            None => 0,
        }
    }
}

#[derive(Default)]
struct Shared {
    job: Option<Arc<Job>>,
    /// 指紋 → 画像。リビジョンをまたいで生き残る。
    cache: HashMap<u64, Arc<Preview>>,
    /// 現在のリビジョンでの フレーム番号 → 指紋（計算済みのもの）。
    keys: HashMap<(i64, u32), u64>,
    failed: HashSet<u64>,
    cache_bytes: usize,
    error: Option<String>,
    shutdown: bool,
}

/// キャッシュの上限。超えたら、いまのリビジョンで使われていないものから捨てる。
const CACHE_BUDGET: usize = 2 << 30;

pub struct PreviewEngine {
    shared: Arc<(Mutex<Shared>, Condvar)>,
    pub footage: Arc<FfmpegFootage>,
    pub clips: Arc<FfmpegClips>,
}

struct DiskCache {
    dir: PathBuf,
}

impl DiskCache {
    fn from_env() -> Option<DiskCache> {
        let dir = PathBuf::from(std::env::var_os("ZND_DISK_CACHE")?);
        std::fs::create_dir_all(&dir).ok()?;
        Some(DiskCache { dir })
    }

    fn path(&self, key: u64) -> PathBuf {
        self.dir.join(format!("{key:016x}.rgba"))
    }

    fn get(&self, key: u64) -> Option<(usize, usize, Vec<u8>)> {
        let data = std::fs::read(self.path(key)).ok()?;
        let (w, h) =
            (u32::from_le_bytes(data.get(0..4)?.try_into().ok()?) as usize, u32::from_le_bytes(data.get(4..8)?.try_into().ok()?) as usize);
        (data.len() == 8 + w * h * 4).then(|| (w, h, data[8..].to_vec()))
    }

    fn put(&self, key: u64, p: &Preview) {
        let mut data = Vec::with_capacity(8 + p.rgba.len());
        data.extend((p.width as u32).to_le_bytes());
        data.extend((p.height as u32).to_le_bytes());
        data.extend(&p.rgba);
        // 書きかけのファイルを読まないよう、別名で書いてから置き換える
        let tmp = self.path(key).with_extension("tmp");
        if std::fs::write(&tmp, data).is_ok() {
            let _ = std::fs::rename(tmp, self.path(key));
        }
    }
}

impl PreviewEngine {
    /// `repaint` は新しいフレームがキャッシュに入るたびに呼ばれる。
    pub fn new(base_dir: &Path, repaint: impl Fn() + Send + 'static) -> Self {
        let shared: Arc<(Mutex<Shared>, Condvar)> = Arc::default();
        let footage = Arc::new(FfmpegFootage::new(base_dir));
        let clips = Arc::new(FfmpegClips::new(base_dir));
        znd_render::effects::set_base_dir(base_dir);
        let (s, f, c, dir) = (shared.clone(), footage.clone(), clips.clone(), base_dir.to_path_buf());
        std::thread::Builder::new()
            .name("znd-preview".into())
            .spawn(move || worker(s, f, c, dir, repaint))
            .expect("failed to spawn preview thread");
        PreviewEngine { shared, footage, clips }
    }

    /// 表示したい状態を伝える。リビジョンが変わったら指紋を計算し直す（キャッシュ自体は残す）。
    pub fn request(&self, project: &Arc<Project>, revision: u64, comp: &CompId, frame: i64, downsample: u32) {
        let (lock, cvar) = &*self.shared;
        let mut s = relock(lock);
        let stale = s.job.as_ref().is_none_or(|j| j.revision != revision || &j.comp != comp);
        let changed = stale || s.job.as_ref().is_none_or(|j| j.frame != frame || j.downsample != downsample);
        if !changed {
            return;
        }
        let (hashes, exprs) = match (&s.job, stale) {
            (Some(j), false) => (j.hashes.clone(), j.exprs.clone()),
            _ => {
                s.keys.clear();
                s.failed.clear();
                s.error = None;
                (Arc::new(Fingerprinter::new(project)), Arc::new(ExprEngine::new()))
            }
        };
        s.job = Some(Arc::new(Job { project: project.clone(), revision, comp: comp.clone(), frame, downsample, hashes, exprs }));
        cvar.notify_all();
    }

    fn key(s: &mut Shared, frame: i64, downsample: u32) -> Option<u64> {
        if let Some(k) = s.keys.get(&(frame, downsample)) {
            return Some(*k);
        }
        let job = s.job.clone()?;
        if job.downsample != downsample {
            return None;
        }
        let k = job.key(frame);
        s.keys.insert((frame, downsample), k);
        Some(k)
    }

    pub fn get(&self, frame: i64, downsample: u32) -> Option<Arc<Preview>> {
        let mut s = relock(&self.shared.0);
        let k = Self::key(&mut s, frame, downsample)?;
        s.cache.get(&k).cloned()
    }

    pub fn is_cached(&self, frame: i64, downsample: u32) -> bool {
        self.get(frame, downsample).is_some()
    }

    /// `frames` のうちキャッシュ済みのもの（タイムラインの緑のバー用）。
    pub fn cached_frames(&self, total: i64, downsample: u32) -> Vec<i64> {
        let mut s = relock(&self.shared.0);
        (0..total).filter(|f| Self::key(&mut s, *f, downsample).is_some_and(|k| s.cache.contains_key(&k))).collect()
    }

    pub fn error(&self) -> Option<String> {
        relock(&self.shared.0).error.clone()
    }
}

impl Drop for PreviewEngine {
    fn drop(&mut self) {
        let (lock, cvar) = &*self.shared;
        relock(lock).shutdown = true;
        cvar.notify_all();
    }
}

/// 次にレンダリングする (フレーム, 指紋)。現在フレームが未キャッシュならそれだけを返す（スクラブの応答性優先）。
fn next_frames(s: &mut Shared, job: &Job, total: i64, batch: usize) -> Vec<(i64, u64)> {
    let current = job.frame.clamp(0, total - 1);
    let mut out: Vec<(i64, u64)> = Vec::new();
    let mut seen = HashSet::new();
    for d in 0..total {
        let f = (current + d) % total;
        let Some(k) = PreviewEngine::key(s, f, job.downsample) else { break };
        // 同じ指紋（＝同じ絵）のフレームは 1 回だけレンダリングすればよい
        if s.cache.contains_key(&k) || s.failed.contains(&k) || !seen.insert(k) {
            continue;
        }
        out.push((f, k));
        if d == 0 || out.len() >= batch {
            break;
        }
        if s.cache_bytes >= CACHE_BUDGET {
            break;
        }
    }
    out
}

fn worker(
    shared: Arc<(Mutex<Shared>, Condvar)>,
    footage: Arc<FfmpegFootage>,
    clips: Arc<FfmpegClips>,
    base_dir: PathBuf,
    repaint: impl Fn(),
) {
    let (lock, cvar) = &*shared;
    let batch = (rayon::current_num_threads() / 2).max(1);
    let disk = DiskCache::from_env();
    loop {
        // やることができるまで待つ
        let (job, frames) = {
            let mut s = relock(lock);
            loop {
                if s.shutdown {
                    return;
                }
                if let Some(job) = s.job.clone() {
                    let total = job.project.comp(&job.comp).map(|c| c.frame_count()).unwrap_or(0);
                    if total > 0 {
                        let frames = next_frames(&mut s, &job, total, batch);
                        if !frames.is_empty() {
                            break (job, frames);
                        }
                    }
                }
                s = cvar.wait(s).unwrap_or_else(|e| e.into_inner());
            }
        };

        let Some(comp) = job.project.comp(&job.comp) else { continue };
        let mixer = Mixer::new(&job.project, &*clips);
        let renderer = Renderer::new(&job.project, &*footage)
            .with_resolver(&*job.exprs)
            .with_audio(&mixer)
            .with_base_dir(&base_dir)
            .with_accel(crate::gpu());
        let xf = renderer.output_xf();
        let opts = job.opts();
        let results: Vec<(u64, Result<Preview, String>)> = frames
            .par_iter()
            .map(|(f, key)| {
                if let Some((width, height, rgba)) = disk.as_ref().and_then(|d| d.get(*key)) {
                    // 検査結果は保存していないので空にする（上映プレビューを開いたフレームは作り直される）
                    let screening = ScreeningReport { clipped_white: 0.0, crushed_black: 0.0, banding_risk: 0.0, overlay: Vec::new() };
                    return (*key, Ok(Preview { width, height, rgba, screening }));
                }
                // エフェクトなどの内部エラーでパニックしても、プレビューのスレッドとアプリは止めない
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    renderer
                        .render_frame(&job.comp, comp.frame_rate.frame_to_time(*f), opts)
                        .map(|buf| Preview {
                            width: buf.width,
                            height: buf.height,
                            rgba: encode_rgba8(&buf, Some(*f as u32), &xf),
                            screening: screening_report(&buf, &xf),
                        })
                        .map_err(|e| e.to_string())
                }))
                .unwrap_or_else(|e| Err(format!("フレーム {f} のレンダリング中に内部エラーが起きました: {}", panic_text(&*e))));
                if let (Some(d), Ok(p)) = (&disk, &r) {
                    d.put(*key, p);
                }
                (*key, r)
            })
            .collect();

        let mut s = relock(lock);
        for (key, r) in results {
            match r {
                Ok(p) => {
                    s.cache_bytes += p.rgba.len() + p.screening.overlay.len();
                    s.cache.insert(key, Arc::new(p));
                }
                Err(e) => {
                    s.failed.insert(key);
                    s.error = Some(e);
                }
            }
        }
        if s.cache_bytes > CACHE_BUDGET {
            // いまのリビジョンのどのフレームにも対応しない画像から捨てる
            let live: HashSet<u64> = s.keys.values().copied().collect();
            let dead: Vec<u64> = s.cache.keys().filter(|k| !live.contains(k)).copied().collect();
            for k in dead {
                if let Some(p) = s.cache.remove(&k) {
                    s.cache_bytes -= p.rgba.len() + p.screening.overlay.len();
                }
                if s.cache_bytes <= CACHE_BUDGET * 3 / 4 {
                    break;
                }
            }
        }
        drop(s);
        repaint();
    }
}

/// パニックの内容を文字列にする。
pub(crate) fn panic_text(e: &(dyn std::any::Any + Send)) -> String {
    e.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| e.downcast_ref::<String>().cloned()).unwrap_or_else(|| "不明なエラー".into())
}
