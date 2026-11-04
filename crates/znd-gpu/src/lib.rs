//! GPU バックエンド（wgpu）。エフェクトを GPU で処理する。
//!
//! レイヤーに掛かっているエフェクトのうち GPU 対応のものが続く区間を、画像を 1 回転送するだけで
//! まとめて処理する（`znd_render::effects::EffectAccel`）。
//!
//! 合成（`znd_render::gpu_composite::CompositeAccel`、[`composite`]）も GPU で行える。レイヤーのソースを送り、
//! エフェクト・配置・描画モードでの重ね合わせまで GPU の上で済ませて、完成したフレームだけを読み戻す。
//!
//! 計算式は CPU リファレンス（`znd-render/src/effects`）と同じで、`tests/parity.rs` が全対応エフェクトを
//! CPU の結果と比較している。対応していないエフェクトやパラメータ（巨大なぼかし半径など）は
//! `supports` が false を返し、CPU で処理される。

mod batch;
mod composite;

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
/// 使い回すために取っておくテクスチャの合計の上限。
const POOL_BYTES: usize = 1536 << 20;

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

/// エフェクトのシェーダー（`shaders.wgsl`）を、コンピュートシェーダーにして返す。
///
/// wgpu の Metal では描画パスごとにコマンドバッファが作られ、レイヤー 200 枚 × 数パスでは遅すぎる。
/// コンピュートなら 1 つのパスに何千回でもディスパッチを積めるので、各 `fs_*` を普通の関数に直し、
/// 画素ごとに呼んで書き込む `cs_*` を足す（計算式はフラグメント版と同じ）。
fn compute_source() -> String {
    let mut src = include_str!("shaders.wgsl").to_string();
    for fs in Fs::ALL {
        let name = fs.entry();
        let from = format!("@fragment\nfn {name}(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {{");
        assert!(src.contains(&from), "{name} の宣言が見つかりません");
        src = src.replace(&from, &format!("fn {name}(pos: vec4<f32>) -> vec4<f32> {{"));
    }
    src.push_str(
        "\n@group(0) @binding(3) var out_tex: texture_storage_2d<rgba32float, write>;\n\n\
         fn out_px(g: vec3<u32>) -> bool {\n  let d = textureDimensions(out_tex);\n  return g.x < d.x && g.y < d.y;\n}\n\n\
         fn center(g: vec3<u32>) -> vec4<f32> {\n  return vec4<f32>(f32(g.x) + 0.5, f32(g.y) + 0.5, 0.0, 1.0);\n}\n\n\
         @compute @workgroup_size(8, 8)\nfn cs_fill(@builtin(global_invocation_id) g: vec3<u32>) {\n  if (out_px(g)) {\n    textureStore(out_tex, vec2<i32>(g.xy), u.v[0]);\n  }\n}\n",
    );
    for fs in Fs::ALL {
        let name = fs.entry();
        let cs = name.replacen("fs_", "cs_", 1);
        src.push_str(&format!(
            "\n@compute @workgroup_size(8, 8)\nfn {cs}(@builtin(global_invocation_id) g: vec3<u32>) {{\n  if (out_px(g)) {{\n    textureStore(out_tex, vec2<i32>(g.xy), {name}(center(g)));\n  }}\n}}\n"
        ));
    }
    src
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
    /// エフェクトの各処理（コンピュートシェーダー。`shaders.wgsl` の各 `fs_*` を包んだもの）。
    pipelines: HashMap<Fs, wgpu::ComputePipeline>,
    /// 画像を 1 色で塗る。
    fill: wgpu::ComputePipeline,
    /// 大きな画像のエフェクト用の描画パス（`shaders.wgsl` のフラグメントシェーダーそのまま）。
    raster: HashMap<Fs, wgpu::RenderPipeline>,
    raster_layout: wgpu::BindGroupLayout,
    /// 合成（レイヤー 1 枚をキャンバスに重ねる、コンピュートシェーダー）。
    place: wgpu::ComputePipeline,
    place_layout: wgpu::BindGroupLayout,
    /// 合成のキャンバスと読み戻し用のバッファ（大きさが同じ間は使い回す）。
    canvas: Mutex<Option<(u32, u32, wgpu::Buffer, wgpu::Buffer)>>,
    layout: wgpu::BindGroupLayout,
    /// 使い終わったテクスチャ（大きさごと）。
    pool: Mutex<HashMap<(u32, u32), Vec<wgpu::Texture>>>,
    max_size: u32,
    name: String,
    composite_lock: Mutex<()>,
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
            source: wgpu::ShaderSource::Wgsl(compute_source().into()),
        });
        let tex_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
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
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: None },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2,
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
        let compute = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: entry,
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let pipelines = Fs::ALL.into_iter().map(|fs| (fs, compute(&fs.entry().replacen("fs_", "cs_", 1)))).collect();
        let fill = compute("cs_fill");
        // 大きな画像のエフェクトは描画パス（フラグメントシェーダー）のほうが速い（タイル型の GPU で
        // テクスチャの読み書きが効率的なため）。パスごとのコストは画素数に比べて無視できる
        let raster_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("znd effects (raster)"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders.wgsl").into()),
        });
        let frag_tex = |binding: u32| wgpu::BindGroupLayoutEntry { visibility: wgpu::ShaderStages::FRAGMENT, ..tex_entry(binding) };
        let raster_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("znd effects (raster)"),
            entries: &[
                frag_tex(0),
                frag_tex(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: None },
                    count: None,
                },
            ],
        });
        let raster_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&raster_layout],
            push_constant_ranges: &[],
        });
        let raster = Fs::ALL
            .into_iter()
            .map(|fs| {
                let p = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(fs.entry()),
                    layout: Some(&raster_pipeline_layout),
                    vertex: wgpu::VertexState { module: &raster_shader, entry_point: "vs", buffers: &[], compilation_options: Default::default() },
                    fragment: Some(wgpu::FragmentState {
                        module: &raster_shader,
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
        let composite = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("znd composite"),
            source: wgpu::ShaderSource::Wgsl(include_str!("composite.wgsl").into()),
        });
        let place_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("znd composite"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: None },
                    count: None,
                },
            ],
        });
        let place = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("cs_place"),
            layout: Some(&device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[&place_layout],
                push_constant_ranges: &[],
            })),
            module: &composite,
            entry_point: "cs_place",
            compilation_options: Default::default(),
            cache: None,
        });
        let max_size = device.limits().max_texture_dimension_2d;
        Ok(GpuBackend {
            device,
            queue,
            pipelines,
            fill,
            raster,
            raster_layout,
            place,
            place_layout,
            canvas: Mutex::default(),
            layout,
            pool: Mutex::default(),
            max_size,
            name: format!("{} ({:?})", info.name, info.backend),
            composite_lock: Mutex::new(()),
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
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    fn recycle(&self, textures: Vec<wgpu::Texture>, w: u32, h: u32) {
        let mut pool = self.pool.lock().unwrap();
        // 合成ではレイヤーごとに大きさが違うので、大きさの種類ではなく合計のメモリ量で上限を決める。
        // 超えたら全部捨てる（プレビュー解像度の切り替えなどで、使わない大きさが溜まったとき）
        let bytes = |w: u32, h: u32, n: usize| w as usize * h as usize * 16 * n;
        let total: usize = pool.iter().map(|((pw, ph), v)| bytes(*pw, *ph, v.len())).sum();
        if total + bytes(w, h, textures.len()) > POOL_BYTES {
            pool.clear();
        }
        let slot = pool.entry((w, h)).or_default();
        for t in textures {
            if slot.len() < 16 {
                slot.push(t);
            }
        }
    }

    fn upload(&self, buf: &FrameBuf) -> wgpu::Texture {
        let (w, h) = (buf.width as u32, buf.height as u32);
        let tex = self.texture(w, h);
        self.queue.write_texture(
            wgpu::ImageCopyTexture { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            bytemuck::cast_slice(&buf.data),
            wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(w * 16), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        tex
    }

    /// 記録した処理を実行して、`tex` を読み戻す。
    fn read_back(&self, mut encoder: wgpu::CommandEncoder, tex: &wgpu::Texture, w: u32, h: u32) -> Result<Vec<f32>, String> {
        let size = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
        // 行の長さは 256 バイトの倍数にそろえる必要がある
        let row = w as usize * 16;
        let padded = row.div_ceil(256) * 256;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (padded * h as usize) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture { texture: tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
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
        let mut out = vec![0.0f32; w as usize * h as usize * 4];
        {
            let mapped = slice.get_mapped_range();
            for y in 0..h as usize {
                let src = &mapped[y * padded..y * padded + row];
                bytemuck::cast_slice_mut::<f32, u8>(&mut out[y * w as usize * 4..(y + 1) * w as usize * 4]).copy_from_slice(src);
            }
        }
        readback.unmap();
        Ok(out)
    }

    fn execute(&self, prog: &Program, result: usize, buf: &FrameBuf) -> Result<Vec<f32>, String> {
        let mut batch = batch::Batch::new(self);
        let input = batch.upload(buf);
        let out = batch.program(prog, result, input);
        let mut encoder = self.device.create_command_encoder(&Default::default());
        batch.record(&mut encoder, None);
        let data = self.read_back(encoder, batch.texture(out), buf.width as u32, buf.height as u32);
        batch.finish();
        data
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
