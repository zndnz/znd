//! GPU の処理を 1 つのコンピュートパスにまとめて記録する。
//!
//! テクスチャのアップロード・塗りつぶし・エフェクトの各処理・合成の配置を順に積んでおき、最後に
//! パラメータを 1 本のバッファ（動的オフセット）にまとめ、コンピュートパスにディスパッチを並べる。
//! 大きな画像（`MIN_PIXELS` 以上）のエフェクトだけは描画パスで処理する（そのほうが速い）。
//! WebGPU ではディスパッチの順に結果が見えるので、前の処理の出力を次の処理で読める。

use crate::{Fs, GpuBackend, Program, PARAM_FLOATS};
use znd_render::FrameBuf;

#[derive(Clone, Copy)]
enum Pipe {
    Fx(Fs),
    Fill,
    Place,
}

struct Op {
    pipe: Pipe,
    /// 読むテクスチャ（`views` の番号）。
    a: usize,
    b: usize,
    /// 書くテクスチャ（配置では使わない）。
    out: usize,
    params: [f32; PARAM_FLOATS],
    groups: [u32; 2],
}

pub(crate) struct Batch<'g> {
    gpu: &'g GpuBackend,
    textures: Vec<(wgpu::Texture, u32, u32)>,
    views: Vec<wgpu::TextureView>,
    ops: Vec<Op>,
    /// 何も読まない処理（塗りつぶし）の入力に使う 1×1 のテクスチャ。
    dummy: Option<usize>,
}

impl<'g> Batch<'g> {
    pub fn new(gpu: &'g GpuBackend) -> Self {
        Batch { gpu, textures: Vec::new(), views: Vec::new(), ops: Vec::new(), dummy: None }
    }

    fn add(&mut self, tex: wgpu::Texture, w: u32, h: u32) -> usize {
        self.views.push(tex.create_view(&Default::default()));
        self.textures.push((tex, w, h));
        self.views.len() - 1
    }

    /// 空のテクスチャ（中身は不定）。
    pub fn alloc(&mut self, w: u32, h: u32) -> usize {
        let t = self.gpu.texture(w, h);
        self.add(t, w, h)
    }

    pub fn upload(&mut self, buf: &FrameBuf) -> usize {
        let t = self.gpu.upload(buf);
        self.add(t, buf.width as u32, buf.height as u32)
    }

    pub fn texture(&self, i: usize) -> &wgpu::Texture {
        &self.textures[i].0
    }

    fn size(&self, i: usize) -> (u32, u32) {
        (self.textures[i].1, self.textures[i].2)
    }

    /// `w`×`h` を `color` で塗ったテクスチャ。
    pub fn fill(&mut self, w: u32, h: u32, color: [f32; 4]) -> usize {
        // 書き込み先を読み込み側にも指定すると使い方が衝突するので、読み込み側は別のテクスチャにする
        let dummy = match self.dummy {
            Some(d) => d,
            None => {
                let d = self.upload(&FrameBuf::new(1, 1));
                self.dummy = Some(d);
                d
            }
        };
        let out = self.alloc(w, h);
        let mut params = [0.0; PARAM_FLOATS];
        params[..4].copy_from_slice(&color);
        self.ops.push(Op { pipe: Pipe::Fill, a: dummy, b: dummy, out, params, groups: [w.div_ceil(8), h.div_ceil(8)] });
        out
    }

    /// エフェクトのプログラムを `input`（レジスタ 0）に対して積む。結果のテクスチャを返す。
    pub fn program(&mut self, prog: &Program, result: usize, input: usize) -> usize {
        if prog.steps.is_empty() {
            return input;
        }
        let (w, h) = self.size(input);
        let regs: Vec<usize> = (0..prog.regs).map(|r| if r == 0 { input } else { self.alloc(w, h) }).collect();
        for step in &prog.steps {
            self.ops.push(Op {
                pipe: Pipe::Fx(step.fs),
                a: regs[step.a],
                b: regs[step.b],
                out: regs[step.out],
                params: step.params,
                groups: [w.div_ceil(8), h.div_ceil(8)],
            });
        }
        regs[result]
    }

    /// 合成の配置（`source` をキャンバスに重ねる）。`region` は処理する範囲の大きさ。
    pub fn place(&mut self, source: usize, params: [f32; PARAM_FLOATS], region: [u32; 2]) {
        self.ops.push(Op { pipe: Pipe::Place, a: source, b: source, out: source, params, groups: [region[0].div_ceil(8), region[1].div_ceil(8)] });
    }

    /// 積んだ処理を `encoder` に 1 つのコンピュートパスとして記録する。配置があるなら `canvas` が要る。
    pub fn record(&self, encoder: &mut wgpu::CommandEncoder, canvas: Option<&wgpu::Buffer>) {
        if self.ops.is_empty() {
            return;
        }
        let device = &self.gpu.device;
        let stride = (PARAM_FLOATS * 4).next_multiple_of(device.limits().min_uniform_buffer_offset_alignment as usize);
        let mut blob = vec![0u8; stride * self.ops.len()];
        for (i, op) in self.ops.iter().enumerate() {
            blob[i * stride..i * stride + PARAM_FLOATS * 4].copy_from_slice(bytemuck::cast_slice(&op.params));
        }
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: blob.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.gpu.queue.write_buffer(&uniforms, 0, &blob);
        let uniform = || {
            wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &uniforms,
                offset: 0,
                size: std::num::NonZeroU64::new((PARAM_FLOATS * 4) as u64),
            })
        };
        // 大きな画像のエフェクトは描画パスで、それ以外はまとめてコンピュートパスで処理する
        let raster = |op: &Op| match op.pipe {
            Pipe::Fx(_) => {
                let (w, h) = self.size(op.out);
                (w as usize * h as usize) >= crate::MIN_PIXELS
            }
            _ => false,
        };
        let binds: Vec<wgpu::BindGroup> = self
            .ops
            .iter()
            .map(|op| {
                let tex = |i: usize| wgpu::BindingResource::TextureView(&self.views[i]);
                match op.pipe {
                    Pipe::Place => device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: None,
                        layout: &self.gpu.place_layout,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: tex(op.a) },
                            wgpu::BindGroupEntry { binding: 1, resource: canvas.expect("配置にはキャンバスが要る").as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 2, resource: uniform() },
                        ],
                    }),
                    _ if raster(op) => device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: None,
                        layout: &self.gpu.raster_layout,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: tex(op.a) },
                            wgpu::BindGroupEntry { binding: 1, resource: tex(op.b) },
                            wgpu::BindGroupEntry { binding: 2, resource: uniform() },
                        ],
                    }),
                    _ => device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: None,
                        layout: &self.gpu.layout,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: tex(op.a) },
                            wgpu::BindGroupEntry { binding: 1, resource: tex(op.b) },
                            wgpu::BindGroupEntry { binding: 2, resource: uniform() },
                            wgpu::BindGroupEntry { binding: 3, resource: tex(op.out) },
                        ],
                    }),
                }
            })
            .collect();
        let mut i = 0;
        while i < self.ops.len() {
            if raster(&self.ops[i]) {
                let (op, Pipe::Fx(fs)) = (&self.ops[i], self.ops[i].pipe) else { unreachable!() };
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.views[op.out],
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&self.gpu.raster[&fs]);
                pass.set_bind_group(0, &binds[i], &[(i * stride) as u32]);
                pass.draw(0..3, 0..1);
                i += 1;
                continue;
            }
            // 次の描画パスまでを 1 つのコンピュートパスに積む
            let mut pass = encoder.begin_compute_pass(&Default::default());
            while i < self.ops.len() && !raster(&self.ops[i]) {
                let op = &self.ops[i];
                pass.set_pipeline(match op.pipe {
                    Pipe::Fx(fs) => &self.gpu.pipelines[&fs],
                    Pipe::Fill => &self.gpu.fill,
                    Pipe::Place => &self.gpu.place,
                });
                pass.set_bind_group(0, &binds[i], &[(i * stride) as u32]);
                pass.dispatch_workgroups(op.groups[0], op.groups[1], 1);
                i += 1;
            }
        }
    }

    pub fn op_count(&self) -> usize {
        self.ops.len()
    }

    /// 使ったテクスチャを使い回しに戻す（実行が終わってから呼ぶ）。
    pub fn finish(self) {
        let gpu = self.gpu;
        drop(self.views);
        for (t, w, h) in self.textures {
            gpu.recycle(vec![t], w, h);
        }
    }
}
