//! フッテージのデコード。静止画は `image`、動画は ffmpeg プロセスから 16bit RGBA を読む。
//!
//! 仮定（M1）: 素材は sRGB/Rec.709 原色・sRGB 伝達関数として扱い、固定フレームレート前提で
//! `floor(秒 × fps)` 番目のフレームを返す。色空間の厳密な扱いと VFR 適合は M2/M3。

use crate::probe::is_still;
use crate::tools;
use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use znd_core::{FootageItem, FrameRate};
use znd_render::color::srgb_to_linear;
use znd_render::{FootageSource, FrameBuf};

/// 連続して読み進めるほうが、シークし直すより速いとみなす最大フレーム数。
const MAX_SKIP_AHEAD: i64 = 24;
const CACHE_BYTES: usize = 1 << 30;

struct Decoder {
    child: Child,
    stdout: ChildStdout,
    /// 次に stdout から出てくるフレーム番号。
    next_frame: i64,
}

impl Drop for Decoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Default)]
struct State {
    decoders: HashMap<PathBuf, Decoder>,
    cache: HashMap<(PathBuf, i64), Arc<FrameBuf>>,
    /// 挿入順（古いものから捨てる）。
    order: VecDeque<(PathBuf, i64)>,
    cache_bytes: usize,
    sequences: HashMap<PathBuf, crate::sequence::Sequence>,
}

impl State {
    fn insert(&mut self, key: (PathBuf, i64), frame: Arc<FrameBuf>) {
        let bytes = frame.data.len() * 4;
        if self.cache.insert(key.clone(), frame).is_none() {
            self.order.push_back(key);
            self.cache_bytes += bytes;
        }
        while self.cache_bytes > CACHE_BYTES && self.order.len() > 1 {
            if let Some(old) = self.order.pop_front() {
                if let Some(f) = self.cache.remove(&old) {
                    self.cache_bytes -= f.data.len() * 4;
                }
            }
        }
    }
}

/// ffmpeg と `image` によるフッテージ供給。相対パスは `base_dir`（プロジェクトファイルの場所）から解決する。
pub struct FfmpegFootage {
    base_dir: PathBuf,
    state: Mutex<State>,
    /// フッテージ ID → プロキシ（低解像度の代替ファイル）とその解像度。
    proxies: Mutex<HashMap<znd_core::FootageId, (PathBuf, u32, u32)>>,
}

impl FfmpegFootage {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        FfmpegFootage { base_dir: base_dir.into(), state: Mutex::new(State::default()), proxies: Mutex::default() }
    }

    pub fn resolve(&self, item: &FootageItem) -> PathBuf {
        resolve_path(&self.base_dir, &item.path)
    }

    /// 以後、このフッテージはプロキシから読む（レイヤー座標は元の解像度のまま）。
    pub fn set_proxy(&self, id: &znd_core::FootageId, path: PathBuf, width: u32, height: u32) {
        self.proxies.lock().unwrap().insert(id.clone(), (path, width, height));
    }

    pub fn clear_proxies(&self) {
        self.proxies.lock().unwrap().clear();
    }

    /// 生成済みのプロキシ（`<基準>/.znd-proxy/<ID>.mp4`）があれば登録する。登録した数を返す。
    pub fn attach_existing_proxies(&self, project: &znd_core::Project) -> usize {
        let mut n = 0;
        for item in &project.footage {
            let path = crate::proxy::proxy_path(&self.base_dir, item);
            if let Ok(info) = crate::probe::probe(&path) {
                self.set_proxy(&item.id, path, info.width, info.height);
                n += 1;
            }
        }
        n
    }
}

/// プロジェクト基準の相対パスを解決する。
pub fn resolve_path(base_dir: &Path, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base_dir.join(p)
    }
}

fn lut16() -> &'static [f32] {
    static LUT: OnceLock<Vec<f32>> = OnceLock::new();
    LUT.get_or_init(|| (0..=65535u32).map(|v| srgb_to_linear(v as f32 / 65535.0)).collect())
}

/// ストレートアルファの 16bit sRGB RGBA（リトルエンディアン）→ リニア・プリマルチプライド。
fn decode_rgba16(bytes: &[u8], width: usize, height: usize) -> FrameBuf {
    let lut = lut16();
    let mut buf = FrameBuf::new(width, height);
    for (dst, src) in buf.data.chunks_exact_mut(4).zip(bytes.chunks_exact(8)) {
        let ch = |i: usize| u16::from_le_bytes([src[2 * i], src[2 * i + 1]]) as usize;
        let a = ch(3) as f32 / 65535.0;
        dst[0] = lut[ch(0)] * a;
        dst[1] = lut[ch(1)] * a;
        dst[2] = lut[ch(2)] * a;
        dst[3] = a;
    }
    buf
}

pub(crate) fn load_still(path: &Path) -> Result<FrameBuf, String> {
    let img = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if matches!(img, image::DynamicImage::ImageRgb32F(_) | image::DynamicImage::ImageRgba32F(_)) {
        // 浮動小数の画像（EXR 等）はリニア値として扱い、1 を超える値もそのまま保つ
        let img = img.into_rgba32f();
        let mut buf = FrameBuf::new(img.width() as usize, img.height() as usize);
        for (dst, src) in buf.data.chunks_exact_mut(4).zip(img.into_raw().chunks_exact(4)) {
            let a = src[3].clamp(0.0, 1.0);
            dst.copy_from_slice(&[src[0] * a, src[1] * a, src[2] * a, a]);
        }
        return Ok(buf);
    }
    let img = img.into_rgba16();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let bytes: Vec<u8> = img.into_raw().iter().flat_map(|v| v.to_le_bytes()).collect();
    Ok(decode_rgba16(&bytes, w, h))
}

fn spawn_decoder(path: &Path, frame: i64, fps: FrameRate, conform: bool) -> Result<Decoder, String> {
    let ffmpeg = tools::ffmpeg().map_err(|e| e.to_string())?;
    // 精密シークは「指定時刻以降の最初のフレーム」から出力するので、目的フレームの半フレーム手前を指定する
    let mut cmd = Command::new(ffmpeg);
    if conform {
        // 可変フレームレート素材: 目的フレームの時刻ちょうどへシークし、そこを起点に固定レートへ並べ直す。
        // 各出力フレームには「その時刻に表示されているフレーム（表示時刻がそれ以前で最も新しいもの）」が選ばれる。
        let seek = frame as f64 / fps.fps();
        cmd.args(["-v", "error", "-nostdin"])
            .args(crate::probe::input_format_args(path))
            .args(["-ss", &format!("{seek:.6}"), "-i"])
            .arg(path)
            .args(["-map", "0:v:0", "-vf", &format!("fps=fps={}/{}:start_time=0:round=near", fps.num, fps.den)]);
    } else {
        let seek = ((frame as f64 - 0.5) / fps.fps()).max(0.0);
        cmd.args(["-v", "error", "-nostdin"])
            .args(crate::probe::input_format_args(path))
            .args(["-ss", &format!("{seek:.6}"), "-i"])
            .arg(path)
            .args(["-map", "0:v:0", "-fps_mode", "passthrough"]);
    }
    let mut child = cmd
        .args(["-f", "rawvideo", "-pix_fmt", "rgba64le", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("ffmpeg を起動できません: {e}"))?;
    let stdout = child.stdout.take().expect("stdout is piped");
    Ok(Decoder { child, stdout, next_frame: frame })
}

impl FootageSource for FfmpegFootage {
    fn frame(&self, item: &FootageItem, seconds: f64) -> Result<Option<Arc<FrameBuf>>, String> {
        // プロキシがあればそちらを読む（解像度だけが違う）
        let proxy = self.proxies.lock().unwrap().get(&item.id).cloned();
        let (path, w, h) = match proxy {
            Some((p, w, h)) => (p, w as usize, h as usize),
            None => (self.resolve(item), item.width as usize, item.height as usize),
        };
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());

        if crate::sequence::is_pattern(&path) {
            let Some(fps) = item.frame_rate else { return Ok(None) };
            if seconds < 0.0 || item.duration.is_some_and(|d| seconds >= d.seconds()) {
                return Ok(None);
            }
            let index = (seconds * fps.fps() + 1e-6).floor() as i64;
            let key = (path.clone(), index);
            if let Some(f) = state.cache.get(&key) {
                return Ok(Some(f.clone()));
            }
            let seq = match state.sequences.get(&path) {
                Some(s) => *s,
                None => {
                    let s = crate::sequence::scan(&path)?;
                    state.sequences.insert(path.clone(), s);
                    s
                }
            };
            // 抜けたコマは、それより前で最も近いコマで埋める
            let mut n = seq.start + index;
            while n > seq.start && !crate::sequence::frame_path(&path, n).exists() {
                n -= 1;
            }
            let frame = Arc::new(load_still(&crate::sequence::frame_path(&path, n))?);
            state.insert(key, frame.clone());
            return Ok(Some(frame));
        }

        let Some(fps) = item.frame_rate.filter(|_| !is_still(&path)) else {
            let key = (path, 0);
            if let Some(f) = state.cache.get(&key) {
                return Ok(Some(f.clone()));
            }
            let frame = Arc::new(load_still(&key.0)?);
            state.insert(key, frame.clone());
            return Ok(Some(frame));
        };

        if seconds < 0.0 || item.duration.is_some_and(|d| seconds >= d.seconds()) {
            return Ok(None);
        }
        // 時刻がフレーム境界ちょうどのとき、浮動小数の誤差で 1 つ前に落ちないよう僅かに進める
        let index = (seconds * fps.fps() + 1e-6).floor() as i64;
        let key = (path.clone(), index);
        if let Some(f) = state.cache.get(&key) {
            return Ok(Some(f.clone()));
        }

        let reusable = state.decoders.get(&path).is_some_and(|d| index >= d.next_frame && index - d.next_frame <= MAX_SKIP_AHEAD);
        if !reusable {
            let dec = spawn_decoder(&path, index, fps, item.vfr)?;
            state.decoders.insert(path.clone(), dec);
        }
        let mut raw = vec![0u8; w * h * 8];
        loop {
            let dec = state.decoders.get_mut(&path).expect("decoder was just inserted or reused");
            if dec.stdout.read_exact(&mut raw).is_err() {
                // ストリーム終端（尺の見積もりより実フレームが少ない等）。透明として扱う。
                state.decoders.remove(&path);
                return Ok(None);
            }
            let n = dec.next_frame;
            dec.next_frame += 1;
            let frame = Arc::new(decode_rgba16(&raw, w, h));
            state.insert((path.clone(), n), frame.clone());
            if n == index {
                return Ok(Some(frame));
            }
        }
    }
}
