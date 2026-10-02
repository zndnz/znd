//! GPU バックエンド（wgpu）。エフェクトを GPU で処理する。
//!
//! レイヤーに掛かっているエフェクトのうち GPU 対応のものが続く区間を、画像を 1 回転送するだけで
//! まとめて処理する（`znd_render::effects::EffectAccel`）。レイヤーのラスタライズと合成は CPU のまま。
//!
//! 計算式は CPU リファレンス（`znd-render/src/effects`）と同じで、`tests/parity.rs` が全対応エフェクトを
//! CPU の結果と比較している。対応していないエフェクトやパラメータ（巨大なぼかし半径など）は
//! `supports` が false を返し、CPU で処理される。

use std::collections::HashMap;
use std::sync::Mutex;
use znd_core::effect::EvalParams;
use znd_render::blur::{box_radii, Edge};
use znd_render::effects::{sigma_for, AccelEnv, AccelOp, EffectAccel};
use znd_render::FrameBuf;

/// GPU で処理するエフェクトの種類。
pub const SUPPORTED: &[&str] = &[
    "gaussian_blur",
    "directional_blur",
    "radial_blur",
    "zoom_blur",
    "glow",
    "bloom",
    "lens_flare",
    "light_rays",
    "drop_shadow",
    "levels",
    "hue_saturation",
    "color_balance",
    "tritone",
    "gradient_map",
    "shadow_highlight",
    "exposure",
    "brightness_contrast",
    "invert",
    "posterize",
    "scanlines",
    "find_edges",
    "rgb_split",
    "wave_warp",
    "polar_coordinates",
    "mirror",
    "motion_tile",
    "chroma_key",
    "luma_key",
    "matte_choker",
];

/// 1 パスで回せるボックスブラーの最大半径。これを超えるぼかしは CPU で処理する。
const MAX_BOX_RADIUS: usize = 256;
/// これより小さい画像は CPU で処理する（実測で、GPU への往復コストと CPU の処理時間が釣り合うあたり）。
pub const MIN_PIXELS: usize = 100_000;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;
const PARAM_FLOATS: usize = 48;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Fs {
    Conv,
    Box,
    Along,
    Color,
    AddLight,
    AddScaled,
    Scaled,
    Shadow,
    Over,
    Flare,
    Warp,
    Edges,
}

impl Fs {
    const ALL: [Fs; 12] = [
        Fs::Conv,
        Fs::Box,
        Fs::Along,
        Fs::Color,
        Fs::AddLight,
        Fs::AddScaled,
        Fs::Scaled,
        Fs::Shadow,
        Fs::Over,
        Fs::Flare,
        Fs::Warp,
        Fs::Edges,
    ];

    fn entry(self) -> &'static str {
        match self {
            Fs::Conv => "fs_conv",
            Fs::Box => "fs_box",
            Fs::Along => "fs_along",
            Fs::Color => "fs_color",
            Fs::AddLight => "fs_add_light",
            Fs::AddScaled => "fs_add_scaled",
            Fs::Scaled => "fs_scaled",
            Fs::Shadow => "fs_shadow",
            Fs::Over => "fs_over",
            Fs::Flare => "fs_flare",
            Fs::Warp => "fs_warp",
            Fs::Edges => "fs_edges",
        }
    }
}

/// 1 回の描画パス。`a`・`b` は入力、`out` は出力のレジスタ（テクスチャ）番号。
struct Step {
    fs: Fs,
    a: usize,
    b: usize,
    out: usize,
    params: [f32; PARAM_FLOATS],
}

/// エフェクトの列を描画パスの列に直したもの。レジスタ 0 が入力画像。
struct Program {
    steps: Vec<Step>,
    regs: usize,
    free: Vec<usize>,
}

fn params(v: &[f32]) -> [f32; PARAM_FLOATS] {
    let mut p = [0.0; PARAM_FLOATS];
    p[..v.len()].copy_from_slice(v);
    p
}

impl Program {
    fn new() -> Program {
        Program { steps: Vec::new(), regs: 1, free: Vec::new() }
    }

    fn alloc(&mut self) -> usize {
        self.free.pop().unwrap_or_else(|| {
            self.regs += 1;
            self.regs - 1
        })
    }

    /// もう読まれないレジスタを再利用に回す（入力のレジスタ 0 は最後まで保持する）。
    fn release(&mut self, r: usize) {
        if r != 0 && !self.free.contains(&r) {
            self.free.push(r);
        }
    }

    fn pass(&mut self, fs: Fs, a: usize, b: usize, v: &[f32]) -> usize {
        let out = self.alloc();
        debug_assert!(out != a && out != b);
        self.steps.push(Step { fs, a, b, out, params: params(v) });
        out
    }

    /// `src` をぼかした結果のレジスタを返す（`src` は書き換えない。何もしない場合は `src` 自身）。
    fn gaussian(&mut self, src: usize, sx: f32, sy: f32, edge: Edge) -> usize {
        let e = if edge == Edge::Clamp { 1.0 } else { 0.0 };
        let mut cur = src;
        for (dx, dy, sigma) in [(1.0, 0.0, sx), (0.0, 1.0, sy)] {
            if sigma <= 0.01 {
                continue;
            }
            let mut step = |prog: &mut Program, fs: Fs, v: &[f32]| {
                let out = prog.pass(fs, cur, cur, v);
                if cur != src {
                    prog.release(cur);
                }
                cur = out;
            };
            if sigma < 2.5 {
                // 真のガウスカーネル（CPU と同じ重み）
                let r = (sigma * 3.0).ceil() as i32;
                let mut kernel: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
                let sum: f32 = kernel.iter().sum();
                kernel.iter_mut().for_each(|k| *k /= sum);
                let mut v = vec![dx, dy, r as f32, e];
                v.extend(kernel);
                step(self, Fs::Conv, &v);
            } else {
                for r in box_radii(sigma) {
                    if r > 0 {
                        step(self, Fs::Box, &[dx, dy, r as f32, e]);
                    }
                }
            }
        }
        cur
    }

    /// 元の画像に `blend`（%）だけ戻す色処理。
    fn color(&mut self, cur: usize, mode: f32, blend: f64, rest: &[f32]) -> usize {
        let mut v = vec![mode, (blend / 100.0).clamp(0.0, 1.0) as f32, 0.0, 0.0];
        v.extend_from_slice(rest);
        self.pass(Fs::Color, cur, cur, &v)
    }

    /// 明るい部分を取り出し、半径を倍々にした `layers` 段のぼかしを重ねる（`multi_blur`）。
    fn glow_light(&mut self, cur: usize, threshold: f32, radius: f64, layers: usize, scale: f32) -> usize {
        let bright = self.color(cur, 8.0, 0.0, &[threshold]);
        let layers = layers.clamp(1, 8);
        let k = 1.0 / layers as f32;
        let mut acc: Option<usize> = None;
        for i in 0..layers {
            let r = radius / 2f64.powi((layers - 1 - i) as i32);
            let sg = sigma_for(r, scale);
            let blurred = self.gaussian(bright, sg, sg, Edge::Zero);
            let next = match acc {
                None => self.pass(Fs::Scaled, blurred, blurred, &[k]),
                Some(a) => {
                    let n = self.pass(Fs::AddScaled, a, blurred, &[k]);
                    self.release(a);
                    n
                }
            };
            if blurred != bright {
                self.release(blurred);
            }
            acc = Some(next);
        }
        self.release(bright);
        acc.expect("layers >= 1")
    }
}

fn rgb(c: [f64; 4]) -> [f32; 3] {
    [c[0] as f32, c[1] as f32, c[2] as f32]
}

/// このぼかし量を GPU で回せるか。
fn sigma_ok(sigma: f32) -> bool {
    sigma < 2.5 || box_radii(sigma).into_iter().all(|r| r <= MAX_BOX_RADIUS)
}

fn supported(op: &AccelOp, env: &AccelEnv) -> bool {
    let (p, s) = (op.params, env.scale);
    match op.kind {
        "gaussian_blur" => sigma_ok(sigma_for(p.f("radius"), s)),
        "glow" | "bloom" => sigma_ok(sigma_for(p.f("radius"), s)),
        "drop_shadow" => sigma_ok(sigma_for(p.f("softness"), s)),
        "shadow_highlight" => sigma_ok(sigma_for(p.f("radius"), s)),
        "matte_choker" => sigma_ok(sigma_for(p.f("choke").abs() * 2.0, s)) && sigma_ok(sigma_for(p.f("blur"), s)),
        kind => SUPPORTED.contains(&kind),
    }
}

/// エフェクト 1 つをパスの列に直す。`cur` は現在の画像、戻り値は結果の画像のレジスタ。
fn emit(prog: &mut Program, op: &AccelOp, env: &AccelEnv, cur: usize, w: usize, h: usize) -> usize {
    let (p, s): (&EvalParams, f32) = (op.params, env.scale);
    let (wf, hf) = (w as f32, h as f32);
    let premul = |name: &str| env.cx.premul(p.c(name));
    match op.kind {
        "gaussian_blur" => {
            let sigma = sigma_for(p.f("radius"), s);
            let (sx, sy) = match p.i("dimensions") {
                1 => (sigma, 0.0),
                2 => (0.0, sigma),
                _ => (sigma, sigma),
            };
            prog.gaussian(cur, sx, sy, if p.b("repeat_edge") { Edge::Clamp } else { Edge::Zero })
        }
        "directional_blur" => {
            let len = p.f("length") as f32 * s;
            let a = (p.f("angle") as f32 - 90.0).to_radians();
            let n = (len.ceil() as usize).clamp(1, 96);
            prog.pass(Fs::Along, cur, cur, &[0.0, n as f32, 0.0, 0.0, a.cos() * len, a.sin() * len])
        }
        "radial_blur" | "zoom_blur" => {
            let c = p.p("center");
            let radial = op.kind == "radial_blur";
            let amount = if radial { (p.f("amount") as f32).to_radians() } else { p.f("amount") as f32 / 100.0 };
            let reach = wf.max(hf) * amount;
            let n = (reach.ceil() as usize).clamp(1, 64);
            let mode = if radial { 1.0 } else { 2.0 };
            prog.pass(Fs::Along, cur, cur, &[mode, n as f32, 0.0, 0.0, c[0] as f32 * s, c[1] as f32 * s, amount])
        }
        "glow" | "bloom" => {
            let glow = op.kind == "glow";
            let layers = if glow { p.f("layers").round() as usize } else { 3 };
            let light = prog.glow_light(cur, p.f("threshold") as f32, p.f("radius"), layers, s);
            let tint = if glow {
                let c = premul("color");
                [c[0], c[1], c[2]]
            } else {
                [1.0; 3]
            };
            let out = prog.pass(Fs::AddLight, cur, light, &[p.f("intensity") as f32, tint[0], tint[1], tint[2]]);
            prog.release(light);
            out
        }
        "lens_flare" => {
            let c = p.p("center");
            let col = premul("color");
            let flare = prog.pass(
                Fs::Flare,
                cur,
                cur,
                &[c[0] as f32 * s, c[1] as f32 * s, p.f("brightness") as f32 / 100.0, 0.0, col[0], col[1], col[2], col[3]],
            );
            let out = prog.pass(Fs::AddLight, cur, flare, &[1.0, 1.0, 1.0, 1.0]);
            prog.release(flare);
            out
        }
        "light_rays" => {
            let c = p.p("center");
            let bright = prog.color(cur, 8.0, 0.0, &[p.f("threshold") as f32]);
            let length = (p.f("length") as f32 / 100.0).clamp(0.0, 0.99);
            let rays = prog.pass(Fs::Along, bright, bright, &[3.0, 48.0, 0.0, 0.0, c[0] as f32 * s, c[1] as f32 * s, length]);
            prog.release(bright);
            let col = premul("color");
            let out = prog.pass(Fs::AddLight, cur, rays, &[p.f("intensity") as f32, col[0], col[1], col[2]]);
            prog.release(rays);
            out
        }
        "drop_shadow" => {
            let a = (p.f("angle") as f32 - 90.0).to_radians();
            let dist = p.f("distance") as f32 * s;
            let col = premul("color");
            let opacity = (p.f("opacity") as f32 / 100.0).clamp(0.0, 1.0);
            let shadow = prog.pass(Fs::Shadow, cur, cur, &[a.cos() * dist, a.sin() * dist, opacity, 0.0, col[0], col[1], col[2], col[3]]);
            let sg = sigma_for(p.f("softness"), s);
            let soft = prog.gaussian(shadow, sg, sg, Edge::Zero);
            if soft != shadow {
                prog.release(shadow);
            }
            let out = prog.pass(Fs::Over, cur, soft, &[]);
            prog.release(soft);
            out
        }
        "levels" => {
            let (ib, iw) = (p.f("in_black") as f32, p.f("in_white") as f32);
            let inv_gamma = 1.0 / (p.f("gamma") as f32).max(0.01);
            let range = (iw - ib).abs().max(1e-6) * (iw - ib).signum();
            prog.color(cur, 0.0, 0.0, &[ib, range, inv_gamma, 0.0, p.f("out_black") as f32, p.f("out_white") as f32])
        }
        "hue_saturation" => {
            prog.color(cur, 1.0, 0.0, &[p.f("hue") as f32, p.f("saturation") as f32 / 100.0, p.f("lightness") as f32 / 100.0])
        }
        "color_balance" => {
            let get = |a: &str, b: &str, c: &str| [p.f(a) as f32 / 100.0, p.f(b) as f32 / 100.0, p.f(c) as f32 / 100.0, 0.0];
            let v = [get("shadow_r", "shadow_g", "shadow_b"), get("mid_r", "mid_g", "mid_b"), get("high_r", "high_g", "high_b")].concat();
            prog.color(cur, 2.0, 0.0, &v)
        }
        "tritone" => {
            let (hi, mid, sh) = (rgb(p.c("highlights")), rgb(p.c("midtones")), rgb(p.c("shadows")));
            prog.color(cur, 3.0, p.f("blend"), &[hi[0], hi[1], hi[2], 0.0, mid[0], mid[1], mid[2], 0.0, sh[0], sh[1], sh[2], 0.0])
        }
        "gradient_map" => {
            let (a, b) = (rgb(p.c("color_a")), rgb(p.c("color_b")));
            prog.color(cur, 4.0, p.f("blend"), &[a[0], a[1], a[2], 0.0, b[0], b[1], b[2], 0.0])
        }
        "shadow_highlight" => {
            let sg = sigma_for(p.f("radius"), s);
            let local = prog.gaussian(cur, sg, sg, Edge::Clamp);
            let v = [13.0, 0.0, 0.0, 0.0, p.f("shadow_amount") as f32 / 100.0, p.f("highlight_amount") as f32 / 100.0];
            let out = prog.pass(Fs::Color, cur, local, &v);
            if local != cur {
                prog.release(local);
            }
            out
        }
        "exposure" => prog.color(cur, 5.0, 0.0, &[2f32.powf(p.f("stops") as f32), p.f("offset") as f32]),
        "brightness_contrast" => {
            let (b, c) = (p.f("brightness") as f32 / 255.0, p.f("contrast") as f32 / 100.0);
            let k = if c >= 0.0 { 1.0 / (1.0 - c).max(0.01) } else { 1.0 + c };
            prog.color(cur, 6.0, 0.0, &[b, k])
        }
        "invert" => prog.color(cur, 7.0, p.f("blend"), &[]),
        "posterize" => prog.color(cur, 9.0, 0.0, &[(p.f("levels") as f32).clamp(2.0, 255.0) - 1.0]),
        "luma_key" => {
            let key_bright = if p.i("mode") == 0 { 1.0 } else { 0.0 };
            prog.color(cur, 10.0, 0.0, &[p.f("threshold") as f32, (p.f("softness") as f32).max(1e-4), key_bright])
        }
        "chroma_key" => {
            let key = rgb(p.c("key_color"));
            let y = 0.2126 * key[0] + 0.7152 * key[1] + 0.0722 * key[2];
            let (kcb, kcr) = ((key[2] - y) / 1.8556, (key[0] - y) / 1.5748);
            let dom = if key[1] >= key[0] && key[1] >= key[2] {
                1
            } else if key[2] >= key[0] {
                2
            } else {
                0
            };
            let v = [
                kcb,
                kcr,
                p.f("tolerance") as f32 * 0.5,
                (p.f("softness") as f32 * 0.5).max(1e-4),
                (p.f("despill") as f32 / 100.0).clamp(0.0, 1.0),
                dom as f32,
                ((dom + 1) % 3) as f32,
                ((dom + 2) % 3) as f32,
            ];
            prog.color(cur, 11.0, 0.0, &v)
        }
        "scanlines" => {
            let spacing = (p.f("spacing") as f32 * s).max(1.0);
            let dark = (p.f("darkness") as f32 / 100.0).clamp(0.0, 1.0);
            let scroll = (p.f("speed") * env.time_seconds) as f32 * s;
            prog.color(cur, 12.0, 0.0, &[spacing, dark, scroll])
        }
        "matte_choker" => {
            let choke = p.f("choke");
            let blur = sigma_for(p.f("blur"), s);
            let mut alpha = cur;
            if choke != 0.0 {
                let sg = sigma_for(choke.abs() * 2.0, s);
                let soft = prog.gaussian(alpha, sg, sg, Edge::Zero);
                let th = if choke > 0.0 { 0.85 } else { 0.15 };
                alpha = prog.color(soft, 14.0, 0.0, &[th]);
                if soft != cur {
                    prog.release(soft);
                }
            }
            if blur > 0.0 {
                let soft = prog.gaussian(alpha, blur, blur, Edge::Zero);
                if soft != alpha && alpha != cur {
                    prog.release(alpha);
                }
                alpha = soft;
            }
            let v = [15.0, 0.0, 0.0, 0.0, if choke < 0.0 { 1.0 } else { 0.0 }];
            let out = prog.pass(Fs::Color, cur, alpha, &v);
            if alpha != cur {
                prog.release(alpha);
            }
            out
        }
        "find_edges" => {
            let t = (p.f("blend") / 100.0).clamp(0.0, 1.0) as f32;
            prog.pass(Fs::Edges, cur, cur, &[if p.b("invert") { 1.0 } else { 0.0 }, t])
        }
        "rgb_split" => {
            let amount = p.f("amount") as f32 * s;
            let a = (p.f("angle") as f32).to_radians();
            let radial = if p.b("radial") { 1.0 } else { 0.0 };
            prog.pass(Fs::Warp, cur, cur, &[0.0, 0.0, 0.0, 0.0, a.cos() * amount, a.sin() * amount, radial, amount])
        }
        "wave_warp" => {
            let height = p.f("height") as f32 * s;
            let width = (p.f("width") as f32 * s).max(0.5);
            let a = (p.f("direction") as f32 - 90.0).to_radians();
            let phase = p.f("phase") as f32 / 360.0 + p.f("speed") as f32 * env.time_seconds as f32;
            prog.pass(Fs::Warp, cur, cur, &[1.0, 0.0, 0.0, 0.0, height, width, a.cos(), a.sin(), phase, p.i("wave_type") as f32])
        }
        "polar_coordinates" => {
            let t = (p.f("interpolation") as f32 / 100.0).clamp(0.0, 1.0);
            prog.pass(Fs::Warp, cur, cur, &[2.0, 0.0, 0.0, 0.0, t, if p.i("mode") == 0 { 1.0 } else { 0.0 }])
        }
        "mirror" => {
            let c = p.p("center");
            let a = (p.f("angle") as f32).to_radians();
            prog.pass(Fs::Warp, cur, cur, &[3.0, 0.0, 0.0, 0.0, c[0] as f32 * s, c[1] as f32 * s, a.cos(), a.sin()])
        }
        "motion_tile" => {
            let c = p.p("center");
            let tw = wf * (p.f("tile_width") as f32 / 100.0).max(0.01);
            let th = hf * (p.f("tile_height") as f32 / 100.0).max(0.01);
            let mirror = if p.b("mirror_edges") { 1.0 } else { 0.0 };
            let v = [4.0, 0.0, 0.0, 0.0, c[0] as f32 * s, c[1] as f32 * s, tw, th, mirror, p.f("phase") as f32 / 360.0];
            prog.pass(Fs::Warp, cur, cur, &v)
        }
        _ => cur,
    }
}

pub struct GpuBackend {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipelines: HashMap<Fs, wgpu::RenderPipeline>,
    layout: wgpu::BindGroupLayout,
    /// 使い終わったテクスチャ（大きさごと）。
    pool: Mutex<HashMap<(u32, u32), Vec<wgpu::Texture>>>,
    max_size: u32,
    name: String,
}

impl GpuBackend {
    /// GPU を初期化する。使える GPU が無ければ `Err`（その場合は CPU だけで動かす）。
    pub fn new() -> Result<GpuBackend, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .ok_or("使える GPU が見つかりません")?;
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("znd"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))
        .map_err(|e| format!("GPU を初期化できません: {e}"))?;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("znd effects"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders.wgsl").into()),
        });
        let tex_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("znd effects"),
            entries: &[
                tex_entry(0),
                tex_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipelines = Fs::ALL
            .into_iter()
            .map(|fs| {
                let p = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(fs.entry()),
                    layout: Some(&pipeline_layout),
                    vertex: wgpu::VertexState { module: &shader, entry_point: "vs", buffers: &[], compilation_options: Default::default() },
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: fs.entry(),
                        targets: &[Some(wgpu::ColorTargetState { format: FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                        compilation_options: Default::default(),
                    }),
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    multiview: None,
                    cache: None,
                });
                (fs, p)
            })
            .collect();
        let max_size = device.limits().max_texture_dimension_2d;
        Ok(GpuBackend {
            device,
            queue,
            pipelines,
            layout,
            pool: Mutex::default(),
            max_size,
            name: format!("{} ({:?})", info.name, info.backend),
        })
    }

    /// GPU の名前（表示用）。
    pub fn name(&self) -> &str {
        &self.name
    }

    fn texture(&self, w: u32, h: u32) -> wgpu::Texture {
        if let Some(t) = self.pool.lock().unwrap().get_mut(&(w, h)).and_then(|v| v.pop()) {
            return t;
        }
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    fn recycle(&self, textures: Vec<wgpu::Texture>, w: u32, h: u32) {
        let mut pool = self.pool.lock().unwrap();
        // 大きさの種類が増えすぎたら捨てる（プレビュー解像度の切り替えなどで溜まる）
        if pool.len() > 8 {
            pool.clear();
        }
        let slot = pool.entry((w, h)).or_default();
        for t in textures {
            if slot.len() < 16 {
                slot.push(t);
            }
        }
    }

    fn execute(&self, prog: &Program, result: usize, buf: &FrameBuf) -> Result<Vec<f32>, String> {
        let (w, h) = (buf.width as u32, buf.height as u32);
        let size = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
        let textures: Vec<wgpu::Texture> = (0..prog.regs).map(|_| self.texture(w, h)).collect();
        let views: Vec<wgpu::TextureView> = textures.iter().map(|t| t.create_view(&Default::default())).collect();
        self.queue.write_texture(
            wgpu::ImageCopyTexture { texture: &textures[0], mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            bytemuck::cast_slice(&buf.data),
            wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(w * 16), rows_per_image: Some(h) },
            size,
        );
        let mut encoder = self.device.create_command_encoder(&Default::default());
        for step in &prog.steps {
            let uniform = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: (PARAM_FLOATS * 4) as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.queue.write_buffer(&uniform, 0, bytemuck::cast_slice(&step.params));
            let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&views[step.a]) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&views[step.b]) },
                    wgpu::BindGroupEntry { binding: 2, resource: uniform.as_entire_binding() },
                ],
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &views[step.out],
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipelines[&step.fs]);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
        // 読み戻し。行の長さは 256 バイトの倍数にそろえる必要がある
        let row = w as usize * 16;
        let padded = row.div_ceil(256) * 256;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (padded * h as usize) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &textures[result],
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &readback,
                layout: wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(padded as u32), rows_per_image: Some(h) },
            },
            size,
        );
        self.queue.submit([encoder.finish()]);
        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv().map_err(|e| e.to_string())?.map_err(|e| format!("GPU からの読み戻しに失敗しました: {e}"))?;
        let mut out = vec![0.0f32; buf.data.len()];
        {
            let mapped = slice.get_mapped_range();
            for y in 0..h as usize {
                let src = &mapped[y * padded..y * padded + row];
                bytemuck::cast_slice_mut::<f32, u8>(&mut out[y * w as usize * 4..(y + 1) * w as usize * 4]).copy_from_slice(src);
            }
        }
        readback.unmap();
        drop(views);
        self.recycle(textures, w, h);
        Ok(out)
    }
}

impl EffectAccel for GpuBackend {
    fn supports(&self, op: &AccelOp, env: &AccelEnv, width: usize, height: usize) -> bool {
        // 小さい画像は転送の往復（数 ms）のほうが高くつくので CPU に任せる
        let fits = width * height >= MIN_PIXELS && width as u32 <= self.max_size && height as u32 <= self.max_size;
        fits && SUPPORTED.contains(&op.kind) && supported(op, env)
    }

    fn run(&self, ops: &[AccelOp], mut buf: FrameBuf, env: &AccelEnv) -> Result<FrameBuf, FrameBuf> {
        let mut prog = Program::new();
        let mut cur = 0;
        for op in ops {
            let next = emit(&mut prog, op, env, cur, buf.width, buf.height);
            if next != cur {
                prog.release(cur);
            }
            cur = next;
        }
        if prog.steps.is_empty() {
            return Ok(buf);
        }
        match self.execute(&prog, cur, &buf) {
            Ok(data) if data.iter().all(|v| v.is_finite()) => {
                buf.data = data;
                Ok(buf)
            }
            Ok(_) => Err(buf),
            Err(e) => {
                eprintln!("GPU: {e}");
                Err(buf)
            }
        }
    }
}
