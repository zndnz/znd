//! 合成の GPU 実装（`znd_render::gpu_composite::CompositeAccel`）。
//!
//! キャンバスはストレージバッファ 1 本。レイヤーごとに、ソースのアップロード（または塗りつぶし）、
//! エフェクト、外接矩形だけの配置（キャンバスをその場で書き換える）を、すべて 1 つのコンピュートパスに積む
//! （[`Batch`]）。描画パスは使わない（タイル型の GPU でパスごとにキャンバス全体を読み書きし、
//! wgpu の Metal ではパスごとにコマンドバッファが作られて遅いため）。最後にキャンバスだけを読み戻す。サンプリングと描画モードの式は CPU（`raster.rs`・`blend.rs`）と同じ（`composite.wgsl`）。

use crate::batch::Batch;
use crate::{emit, supported, GpuBackend, Program, PARAM_FLOATS, SUPPORTED};
use znd_core::Affine;
use znd_render::effects::{AccelEnv, AccelOp};
use znd_render::gpu_composite::{CompositeAccel, CompositeJob, CompositeSource};
use znd_render::FrameBuf;

/// `raster.rs` の `MAX_SUPERSAMPLE` と同じ。
const MAX_SUPERSAMPLE: f32 = 4.0;

/// `rasterize` と同じ外接矩形（出力画素、[x0, x1) × [y0, y1)）。
fn bbox(m: &Affine, sw: f32, sh: f32, out_w: usize, out_h: usize) -> Option<[usize; 4]> {
    let corners = [(0.0, 0.0), (sw as f64, 0.0), (0.0, sh as f64), (sw as f64, sh as f64)].map(|(x, y)| m.apply(x, y));
    let min_x = corners.iter().map(|c| c.0).fold(f64::MAX, f64::min).floor() - 1.0;
    let max_x = corners.iter().map(|c| c.0).fold(f64::MIN, f64::max).ceil() + 1.0;
    let min_y = corners.iter().map(|c| c.1).fold(f64::MAX, f64::min).floor() - 1.0;
    let max_y = corners.iter().map(|c| c.1).fold(f64::MIN, f64::max).ceil() + 1.0;
    let x0 = min_x.clamp(0.0, out_w as f64) as usize;
    let x1 = max_x.clamp(0.0, out_w as f64) as usize;
    let y0 = min_y.clamp(0.0, out_h as f64) as usize;
    let y1 = max_y.clamp(0.0, out_h as f64) as usize;
    (x1 > x0 && y1 > y0).then_some([x0, y0, x1, y1])
}

impl CompositeAccel for GpuBackend {
    fn supports_in_composite(&self, op: &AccelOp, env: &AccelEnv) -> bool {
        SUPPORTED.contains(&op.kind) && supported(op, env)
    }

    fn fits(&self, width: usize, height: usize) -> bool {
        width > 0 && height > 0 && width as u32 <= self.max_size && height as u32 <= self.max_size
    }

    fn composite(&self, job: &CompositeJob) -> Result<FrameBuf, String> {
        // GPU のキューは 1 本なので、複数のスレッドから来ても 1 フレームずつ処理する
        let _one_at_a_time = self.composite_lock.lock().unwrap_or_else(|e| e.into_inner());
        let trace = std::env::var_os("ZND_GPU_TRACE").is_some();
        let started = std::time::Instant::now();
        let (w, h) = (job.width as u32, job.height as u32);
        let bytes = w as u64 * h as u64 * 16;
        let mut cached = self.canvas.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(&*cached, Some((cw, ch, _, _)) if *cw == w && *ch == h) {
            let canvas = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("znd canvas"),
                size: bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("znd canvas readback"),
                size: bytes,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            *cached = Some((w, h, canvas, readback));
        }
        let (_, _, canvas, readback) = cached.as_ref().expect("作ったばかり");

        let mut batch = Batch::new(self);
        let mut solid_source: Option<usize> = None;
        let flags = (job.linear as u32) | ((job.clamp as u32) << 1);
        for layer in &job.layers {
            let mut v = [0.0f32; PARAM_FLOATS];
            let stencil = matches!(layer.blend, znd_core::BlendMode::StencilAlpha | znd_core::BlendMode::StencilLuma);
            let (source, rect) = match &layer.source {
                CompositeSource::Tile(tile) => {
                    if tile.w == 0 || tile.h == 0 {
                        continue;
                    }
                    let tex = batch.alloc(tile.w as u32, tile.h as u32);
                    self.queue.write_texture(
                        wgpu::ImageCopyTexture { texture: batch.texture(tex), mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                        bytemuck::cast_slice(&tile.data),
                        wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(tile.w as u32 * 16), rows_per_image: Some(tile.h as u32) },
                        wgpu::Extent3d { width: tile.w as u32, height: tile.h as u32, depth_or_array_layers: 1 },
                    );
                    v[16] = 2.0;
                    (tex, [tile.x0, tile.y0, tile.x0 + tile.w, tile.y0 + tile.h])
                }
                src => {
                    let (sw, sh) = match src {
                        CompositeSource::Solid { width, height, .. } => (*width as f32, *height as f32),
                        CompositeSource::Image { buf, .. } => (buf.width as f32, buf.height as f32),
                        CompositeSource::Fill { width, height, .. } => (*width as f32, *height as f32),
                        CompositeSource::Tile(_) => unreachable!(),
                    };
                    if sw <= 0.0 || sh <= 0.0 || layer.opacity <= 0.0 {
                        continue;
                    }
                    let Some(inv) = layer.matrix.inverse() else { continue };
                    let Some(rect) = bbox(&layer.matrix, sw, sh, job.width, job.height) else { continue };
                    let fx = (inv.a.abs() + inv.c.abs()) as f32;
                    let fy = (inv.b.abs() + inv.d.abs()) as f32;
                    let nx = ((inv.a.hypot(inv.b)) as f32).ceil().clamp(1.0, MAX_SUPERSAMPLE);
                    let ny = ((inv.c.hypot(inv.d)) as f32).ceil().clamp(1.0, MAX_SUPERSAMPLE);
                    v[..8].copy_from_slice(&[inv.a as f32, inv.b as f32, inv.c as f32, inv.d as f32, inv.tx as f32, inv.ty as f32, sw, sh]);
                    v[8..12].copy_from_slice(&[fx, fy, nx, ny]);
                    v[17] = layer.opacity;
                    let tex = match src {
                        CompositeSource::Solid { color, .. } => {
                            v[12..16].copy_from_slice(color);
                            v[16] = 0.0;
                            // 単色は色をパラメータで渡すので、ソースのテクスチャは読まない（何でもよい）
                            *solid_source.get_or_insert_with(|| batch.upload(&FrameBuf::new(1, 1)))
                        }
                        CompositeSource::Image { ops, effect_scale, .. } | CompositeSource::Fill { ops, effect_scale, .. } => {
                            v[16] = 1.0;
                            let input = match src {
                                CompositeSource::Image { buf, .. } => batch.upload(buf),
                                CompositeSource::Fill { width, height, color, .. } => batch.fill(*width as u32, *height as u32, *color),
                                _ => unreachable!(),
                            };
                            // エフェクトは画像の大きさのまま GPU で適用し、その結果を置く
                            let env = job.env(*effect_scale);
                            let mut prog = Program::new();
                            let mut cur = 0;
                            for (kind, p) in ops {
                                let next = emit(&mut prog, &AccelOp { kind, params: p }, &env, cur, sw as usize, sh as usize);
                                if next != cur {
                                    prog.release(cur);
                                }
                                cur = next;
                            }
                            batch.program(&prog, cur, input)
                        }
                        CompositeSource::Tile(_) => unreachable!(),
                    };
                    (tex, rect)
                }
            };
            // ステンシルはレイヤーの外も消すので全面を処理する
            let region = if stencil { [0, 0, job.width, job.height] } else { rect };
            v[18] = layer.blend as u32 as f32;
            v[19] = flags as f32;
            v[20..24].copy_from_slice(&rect.map(|x| x as f32));
            v[24..28].copy_from_slice(&region.map(|x| x as f32));
            v[28] = job.width as f32;
            batch.place(source, v, [(region[2] - region[0]) as u32, (region[3] - region[1]) as u32]);
        }

        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.clear_buffer(canvas, 0, None);
        batch.record(&mut encoder, Some(canvas));
        encoder.copy_buffer_to_buffer(canvas, 0, readback, 0, bytes);
        let encoded = started.elapsed();
        self.queue.submit([encoder.finish()]);
        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv().map_err(|e| e.to_string())?.map_err(|e| format!("GPU からの読み戻しに失敗しました: {e}"))?;
        let data: Vec<f32> = bytemuck::cast_slice(&slice.get_mapped_range()).to_vec();
        readback.unmap();
        if trace {
            eprintln!(
                "GPU 合成: レイヤー {} 枚 / 処理 {} 回 / 記録 {:.1} ms / 実行と読み戻し {:.1} ms",
                job.layers.len(),
                batch.op_count(),
                encoded.as_secs_f64() * 1000.0,
                (started.elapsed() - encoded).as_secs_f64() * 1000.0
            );
        }
        batch.finish();
        if !data.iter().all(|v| v.is_finite()) {
            return Err("GPU の合成結果に不正な値があります".into());
        }
        Ok(FrameBuf { width: job.width, height: job.height, data })
    }
}
