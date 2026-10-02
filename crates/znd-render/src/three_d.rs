//! 3D: 2.5D レイヤー、カメラ（被写界深度）、ライトと影、3D モデルとの深度合成。
//!
//! 座標系はコンポ座標の延長: X 右、Y 下、Z 奥（カメラから遠ざかる方向が正）。
//! カメラが無いコンポでは既定カメラ（50mm 相当、Z=0 の面が等倍）を使うので、
//! Z=0・無回転の 3D レイヤーは 2D レイヤーと同じ位置に描かれる。

use crate::blend::blend_pixel;
use crate::blur::{gaussian, Edge};
use crate::color::ColorXf;
use crate::frame::FrameBuf;
use crate::model::{load_model_at, shade_pbr, Model};
use rayon::prelude::*;
use std::path::Path;
use std::sync::Arc;
use znd_core::{Affine, BlendMode, Composition, Layer, LayerKind, LightType, Time};

pub type V3 = [f64; 3];

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub fn mul(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub fn norm(a: V3) -> V3 {
    let l = dot(a, a).sqrt();
    if l < 1e-12 {
        [0.0, 0.0, 1.0]
    } else {
        mul(a, 1.0 / l)
    }
}

/// 3x4 のアフィン変換（回転・拡縮 3x3 と平行移動）。
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Xf {
    pub m: [V3; 3],
    pub t: V3,
}

impl Xf {
    pub const IDENTITY: Xf = Xf { m: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t: [0.0; 3] };

    pub fn translate(t: V3) -> Xf {
        Xf { t, ..Xf::IDENTITY }
    }

    pub fn scale(s: V3) -> Xf {
        Xf { m: [[s[0], 0.0, 0.0], [0.0, s[1], 0.0], [0.0, 0.0, s[2]]], t: [0.0; 3] }
    }

    /// Z 軸まわり（画面内の回転。2D の回転と同じ向き）。
    pub fn rot_z(deg: f64) -> Xf {
        let (s, c) = deg.to_radians().sin_cos();
        Xf { m: [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]], t: [0.0; 3] }
    }

    /// Y 軸まわり。正の角度で右端が奥へ行く。
    pub fn rot_y(deg: f64) -> Xf {
        let (s, c) = deg.to_radians().sin_cos();
        Xf { m: [[c, 0.0, -s], [0.0, 1.0, 0.0], [s, 0.0, c]], t: [0.0; 3] }
    }

    /// X 軸まわり。正の角度で上端が奥へ行く。
    pub fn rot_x(deg: f64) -> Xf {
        let (s, c) = deg.to_radians().sin_cos();
        Xf { m: [[1.0, 0.0, 0.0], [0.0, c, s], [0.0, -s, c]], t: [0.0; 3] }
    }

    pub fn dir(&self, v: V3) -> V3 {
        [dot(self.m[0], v), dot(self.m[1], v), dot(self.m[2], v)]
    }

    pub fn point(&self, p: V3) -> V3 {
        add(self.dir(p), self.t)
    }

    /// `self ∘ o`（先に `o`）。
    pub fn then_after(&self, o: &Xf) -> Xf {
        let col = |j: usize| self.dir([o.m[0][j], o.m[1][j], o.m[2][j]]);
        let (c0, c1, c2) = (col(0), col(1), col(2));
        Xf { m: [[c0[0], c1[0], c2[0]], [c0[1], c1[1], c2[1]], [c0[2], c1[2], c2[2]]], t: self.point(o.t) }
    }
}

/// レイヤー座標 → 親座標（3D）。
pub fn local_xf(layer: &Layer, t: Time) -> Xf {
    let tr = &layer.transform;
    let (a, p, s) = (tr.anchor.eval(t), tr.position.eval(t), tr.scale.eval(t));
    Xf::translate([p[0], p[1], tr.position_z.eval(t)])
        .then_after(&Xf::rot_z(tr.rotation.eval(t)))
        .then_after(&Xf::rot_y(tr.rotation_y.eval(t)))
        .then_after(&Xf::rot_x(tr.rotation_x.eval(t)))
        .then_after(&Xf::scale([s[0] / 100.0, s[1] / 100.0, 1.0]))
        .then_after(&Xf::translate([-a[0], -a[1], -tr.anchor_z.eval(t)]))
}

/// レイヤー座標 → ワールド座標（親チェーン込み）。
pub fn world_xf(comp: &Composition, layer: &Layer, t: Time) -> Xf {
    let mut m = local_xf(layer, t);
    let mut cur = layer.parent.as_ref();
    for _ in 0..comp.layers.len() {
        let Some(parent) = cur.and_then(|id| comp.layer(id)) else { break };
        m = local_xf(parent, t).then_after(&m);
        cur = parent.parent.as_ref();
    }
    m
}

/// 既定カメラのズーム値（px）。35mm フルサイズで 50mm レンズ相当の画角。
pub fn default_zoom(comp: &Composition) -> f64 {
    comp.width as f64 * 50.0 / 36.0
}

/// 3D シーンに属するレイヤー（3D スイッチの入った描画レイヤー、または 3D モデル）。
pub fn is_scene_layer(layer: &Layer) -> bool {
    match &layer.kind {
        LayerKind::Model { .. } => true,
        LayerKind::Solid { .. }
        | LayerKind::Footage { .. }
        | LayerKind::Precomp { .. }
        | LayerKind::Shape { .. }
        | LayerKind::Text { .. } => layer.three_d,
        _ => false,
    }
}

/// 3D レイヤーの連なりを分断するレイヤー（2D の描画レイヤーと調整レイヤー）。
pub fn breaks_run(layer: &Layer) -> bool {
    match &layer.kind {
        LayerKind::Solid { .. }
        | LayerKind::Footage { .. }
        | LayerKind::Precomp { .. }
        | LayerKind::Shape { .. }
        | LayerKind::Text { .. } => !layer.three_d,
        LayerKind::Adjustment => true,
        _ => false,
    }
}

#[derive(Debug, Clone)]
pub struct Camera {
    pub pos: V3,
    pub right: V3,
    pub down: V3,
    pub forward: V3,
    pub zoom: f64,
    pub dof: Option<Dof>,
}

#[derive(Debug, Copy, Clone)]
pub struct Dof {
    pub focus: f64,
    pub aperture: f64,
    pub iris_sides: u32,
}

impl Camera {
    /// ワールド座標 → カメラ座標（x 右、y 下、z 視線方向の距離）。
    pub fn view(&self, p: V3) -> V3 {
        let d = sub(p, self.pos);
        [dot(d, self.right), dot(d, self.down), dot(d, self.forward)]
    }
}

#[derive(Debug, Clone)]
pub struct Light {
    pub kind: LightType,
    pub pos: V3,
    pub dir: V3,
    /// 作業空間リニアの色 × 強度。
    pub color: [f32; 3],
    pub cone_cos: (f64, f64),
    pub casts_shadows: bool,
    pub shadow_darkness: f32,
}

pub enum SceneItem<'a> {
    Plane {
        layer: &'a Layer,
        buf: Arc<FrameBuf>,
        /// 画像が表すレイヤー座標上の大きさ。
        width: f64,
        height: f64,
        opacity: f32,
        /// 画像の左上のレイヤー座標。
        origin: [f64; 2],
    },
    Model {
        layer: &'a Layer,
        path: String,
        opacity: f32,
    },
}

pub struct Scene {
    pub camera: Camera,
    pub lights: Vec<Light>,
    /// コンポ座標 → 出力画素（プレビュー解像度の縮小）。
    pub base: Affine,
    pub center: [f64; 2],
    pub out_w: usize,
    pub out_h: usize,
}

/// 1 アイテムを描いた結果: 色（プリマルチプライド）と、画素ごとのカメラ距離。
struct Rendered {
    color: Vec<f32>,
    depth: Vec<f32>,
    center_depth: f64,
    blend: BlendMode,
}

impl Scene {
    pub fn new(comp: &Composition, t: Time, base: Affine, out_w: usize, out_h: usize, cx: &ColorXf) -> Scene {
        let center = [comp.width as f64 / 2.0, comp.height as f64 / 2.0];
        let mut camera = Camera {
            pos: [center[0], center[1], -default_zoom(comp)],
            right: [1.0, 0.0, 0.0],
            down: [0.0, 1.0, 0.0],
            forward: [0.0, 0.0, 1.0],
            zoom: default_zoom(comp),
            dof: None,
        };
        // いちばん上にある有効なカメラレイヤーがアクティブカメラ
        for layer in &comp.layers {
            let LayerKind::Camera { zoom, point_of_interest, depth_of_field, focus_distance, aperture, iris_sides } = &layer.kind else {
                continue;
            };
            if !layer.visible || !layer.active_at(t) {
                continue;
            }
            let w = world_xf(comp, layer, t);
            let pos = w.t;
            let (right, down, forward) = match point_of_interest {
                Some(poi) => {
                    // 2 ノード: 目標点を向き、Z 回転でロールする
                    let f = norm(sub(poi.eval(t), pos));
                    let r0 = norm(cross([0.0, 1.0, 0.0], f));
                    let d0 = cross(f, r0);
                    let (s, c) = layer.transform.rotation.eval(t).to_radians().sin_cos();
                    (add(mul(r0, c), mul(d0, s)), add(mul(d0, c), mul(r0, -s)), f)
                }
                None => (norm(w.dir([1.0, 0.0, 0.0])), norm(w.dir([0.0, 1.0, 0.0])), norm(w.dir([0.0, 0.0, 1.0]))),
            };
            camera = Camera {
                pos,
                right,
                down,
                forward,
                zoom: zoom.eval(t),
                dof: depth_of_field.then(|| Dof { focus: focus_distance.eval(t), aperture: aperture.eval(t), iris_sides: *iris_sides }),
            };
            break;
        }
        let lights = comp
            .layers
            .iter()
            .filter(|l| l.visible && l.active_at(t))
            .filter_map(|layer| {
                let LayerKind::Light { light, color, intensity, cone_angle, cone_feather, casts_shadows, shadow_darkness, .. } =
                    &layer.kind
                else {
                    return None;
                };
                let w = world_xf(comp, layer, t);
                let c = cx.rgb(*color);
                let k = (intensity.eval(t) / 100.0) as f32;
                let half = (cone_angle.eval(t) / 2.0).to_radians();
                let feather = (cone_feather.eval(t) / 100.0).clamp(0.0, 1.0);
                Some(Light {
                    kind: *light,
                    pos: w.t,
                    dir: norm(w.dir([0.0, 0.0, 1.0])),
                    color: [c[0] * k, c[1] * k, c[2] * k],
                    // (外側の境界, 内側の境界) の余弦。フェザーが大きいほど内側が狭まる
                    cone_cos: (half.cos(), (half * (1.0 - feather)).cos()),
                    casts_shadows: *casts_shadows,
                    shadow_darkness: (*shadow_darkness / 100.0) as f32,
                })
            })
            .collect();
        Scene { camera, lights, base, center, out_w, out_h }
    }

    /// カメラ座標 → 出力画素。
    fn project(&self, pc: V3) -> (f64, f64) {
        let (x, y) = (self.center[0] + self.camera.zoom * pc[0] / pc[2], self.center[1] + self.camera.zoom * pc[1] / pc[2]);
        self.base.apply(x, y)
    }

    /// ワールド座標 `p`・法線 `n` の点が受ける光（色ごとの係数）。ライトが無ければ 1。
    fn lighting(&self, p: V3, n: V3, shadow: &dyn Fn(&Light, V3) -> f32) -> [f32; 3] {
        if self.lights.is_empty() {
            return [1.0; 3];
        }
        let mut sum = [0.0f32; 3];
        for light in &self.lights {
            let k = match light.kind {
                LightType::Ambient => 1.0,
                LightType::Parallel => dot(n, mul(light.dir, -1.0)).abs() as f32,
                LightType::Point | LightType::Spot => {
                    let l = norm(sub(light.pos, p));
                    let mut k = dot(n, l).abs();
                    if light.kind == LightType::Spot {
                        let c = dot(mul(l, -1.0), light.dir);
                        let (outer, inner) = light.cone_cos;
                        k *= if inner > outer {
                            ((c - outer) / (inner - outer)).clamp(0.0, 1.0)
                        } else if c >= outer {
                            1.0
                        } else {
                            0.0
                        };
                    }
                    k as f32
                }
            };
            let sh = if light.casts_shadows && light.kind != LightType::Ambient { shadow(light, p) } else { 1.0 };
            (0..3).for_each(|c| sum[c] += light.color[c] * k * sh);
        }
        sum
    }

    /// 平面レイヤーを描く。
    fn render_plane(
        &self,
        layer: &Layer,
        buf: &FrameBuf,
        w: f64,
        h: f64,
        origin: [f64; 2],
        opacity: f32,
        xf: &Xf,
        casters: &[Caster],
    ) -> Option<Rendered> {
        // レイヤー座標 (u, v) の点のカメラ座標: O + u·U + v·V
        let o = self.camera.view(xf.point([origin[0], origin[1], 0.0]));
        let u = self.camera.view(xf.point([origin[0] + 1.0, origin[1], 0.0]));
        let v = self.camera.view(xf.point([origin[0], origin[1] + 1.0, 0.0]));
        let (u, v) = (sub(u, o), sub(v, o));
        // 平面上の点 P(u,v) を通る視線: 画素 (x, y) → カメラ座標での方向 d = ((x-cx)/zoom, (y-cy)/zoom, 1)
        // P = s·d を解く: [U V -d]·(a, b, s)ᵀ = -O
        let n_cam = cross(u, v);
        let center_cam = add(o, add(mul(u, w / 2.0), mul(v, h / 2.0)));
        if dot(n_cam, n_cam) < 1e-18 {
            // 真横を向いていて面積が無い
            return None;
        }
        let inv_base = self.base.inverse()?;
        let normal_w = norm(cross(xf.dir([1.0, 0.0, 0.0]), xf.dir([0.0, 1.0, 0.0])));
        let lit = layer.accepts_lights && !self.lights.is_empty();
        let (ow, oh) = (self.out_w, self.out_h);
        let mut color = vec![0.0f32; ow * oh * 4];
        let mut depth = vec![f32::INFINITY; ow * oh];
        let (sx, sy) = (buf.width as f64 / w, buf.height as f64 / h);
        let layer_id = &layer.id;

        // バウンディングボックス（全頂点がカメラの前にあるときだけ絞る）
        let corners = [[0.0, 0.0], [w, 0.0], [0.0, h], [w, h]].map(|c| add(o, add(mul(u, c[0]), mul(v, c[1]))));
        let (mut y0, mut y1) = (0usize, oh);
        if corners.iter().all(|c| c[2] > 1.0) {
            let ys: Vec<f64> = corners.iter().map(|c| self.project(*c).1).collect();
            y0 = (ys.iter().cloned().fold(f64::MAX, f64::min).floor() - 1.0).clamp(0.0, oh as f64) as usize;
            y1 = (ys.iter().cloned().fold(f64::MIN, f64::max).ceil() + 1.0).clamp(0.0, oh as f64) as usize;
        }

        color.par_chunks_mut(ow * 4).zip(depth.par_chunks_mut(ow)).enumerate().filter(|(y, _)| *y >= y0 && *y < y1).for_each(
            |(y, (crow, drow))| {
                for x in 0..ow {
                    // 2x2 のスーパーサンプリングで縁をアンチエイリアスする
                    let mut acc = [0.0f32; 4];
                    let mut z_hit = f32::INFINITY;
                    let mut world_hit = None;
                    for (jx, jy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                        let (cxp, cyp) = inv_base.apply(x as f64 + jx, y as f64 + jy);
                        let d = [(cxp - self.center[0]) / self.camera.zoom, (cyp - self.center[1]) / self.camera.zoom, 1.0];
                        // 視線と平面の交点: s = (O·N) / (d·N)
                        let denom = dot(d, n_cam);
                        if denom.abs() < 1e-12 {
                            continue;
                        }
                        let s = dot(o, n_cam) / denom;
                        if s <= 1.0 {
                            continue;
                        }
                        let p = sub(mul(d, s), o);
                        // U, V は直交とは限らないので、2x2 の連立方程式で (a, b) を解く
                        let (uu, uv, vv) = (dot(u, u), dot(u, v), dot(v, v));
                        let det = uu * vv - uv * uv;
                        if det.abs() < 1e-18 {
                            continue;
                        }
                        let (pu, pv) = (dot(p, u), dot(p, v));
                        let (a, b) = ((pu * vv - pv * uv) / det, (pv * uu - pu * uv) / det);
                        if a < 0.0 || b < 0.0 || a > w || b > h {
                            continue;
                        }
                        let smp = crate::effects::sample_clamped(buf, (a * sx) as f32, (b * sy) as f32);
                        (0..4).for_each(|k| acc[k] += smp[k] * 0.25);
                        if (s as f32) < z_hit {
                            z_hit = s as f32;
                            world_hit = Some(xf.point([origin[0] + a, origin[1] + b, 0.0]));
                        }
                    }
                    if acc[3] <= 0.0 {
                        continue;
                    }
                    let mut px = acc.map(|c| c * opacity);
                    if lit {
                        if let Some(pw) = world_hit {
                            let shadow = |light: &Light, p: V3| shadow_factor(light, p, layer_id, casters);
                            let l = self.lighting(pw, normal_w, &shadow);
                            (0..3).for_each(|k| px[k] *= l[k]);
                        }
                    }
                    crow[x * 4..x * 4 + 4].copy_from_slice(&px);
                    drow[x] = z_hit;
                }
            },
        );

        let mut r = Rendered { color, depth, center_depth: center_cam[2], blend: layer.blend_mode };
        self.depth_of_field(&mut r);
        Some(r)
    }

    /// 被写界深度: アイテム中心の距離から錯乱円の大きさを求め、その分だけぼかす。
    fn depth_of_field(&self, r: &mut Rendered) {
        let Some(dof) = self.camera.dof else { return };
        if r.center_depth <= 1.0 {
            return;
        }
        // 薄レンズ近似: 錯乱円の直径 ∝ 絞り × |距離 − 合焦距離| / 距離
        let coc = dof.aperture * (r.center_depth - dof.focus).abs() / r.center_depth;
        let radius = (coc / 2.0) * crate::compositor::matrix_scale(&self.base) as f64;
        if radius < 0.3 {
            return;
        }
        let (w, h) = (self.out_w, self.out_h);
        if dof.iris_sides >= 3 {
            let buf = FrameBuf { width: w, height: h, data: std::mem::take(&mut r.color) };
            r.color = crate::effects::iris_blur(&buf, radius as f32, dof.iris_sides as f32).data;
        } else {
            let sigma = (radius / 2.0) as f32;
            gaussian(&mut r.color, w, h, 4, sigma, sigma, Edge::Zero);
        }
    }

    /// 3D モデルを描く。
    fn render_model(&self, layer: &Layer, model: &Model, opacity: f32, xf: &Xf) -> Option<Rendered> {
        let (ow, oh) = (self.out_w, self.out_h);
        // 2 倍の解像度で描いて縮小する（輪郭のアンチエイリアス）
        const SS: usize = 2;
        let (sw, sh) = (ow * SS, oh * SS);
        let mut color = vec![0.0f32; sw * sh * 4];
        let mut depth = vec![f32::INFINITY; sw * sh];
        let headlight = [Light {
            kind: LightType::Parallel,
            pos: self.camera.pos,
            dir: self.camera.forward,
            color: [1.0; 3],
            cone_cos: (0.0, 0.0),
            casts_shadows: false,
            shadow_darkness: 0.0,
        }];
        let lights: &[Light] = if self.lights.is_empty() { &headlight } else { &self.lights };
        let ambient = if self.lights.is_empty() { 0.25 } else { 0.0 };

        for mesh in &model.meshes {
            // 頂点をワールド → カメラ → 画面へ
            let world: Vec<V3> = mesh.positions.iter().map(|p| xf.point(*p)).collect();
            let normals: Vec<V3> = mesh.normals.iter().map(|n| norm(xf.dir(*n))).collect();
            let cam: Vec<V3> = world.iter().map(|p| self.camera.view(*p)).collect();
            let screen: Vec<(f64, f64)> = cam
                .iter()
                .map(|c| {
                    let (x, y) = self.project([c[0], c[1], c[2].max(1e-3)]);
                    (x * SS as f64, y * SS as f64)
                })
                .collect();
            for tri in mesh.indices.chunks_exact(3) {
                let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
                // カメラの後ろに回る三角形は描かない（近接クリップは省略）
                if cam[i0][2] <= 1.0 || cam[i1][2] <= 1.0 || cam[i2][2] <= 1.0 {
                    continue;
                }
                let (a, b, c) = (screen[i0], screen[i1], screen[i2]);
                let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
                if area.abs() < 1e-9 {
                    continue;
                }
                let (min_x, max_x) =
                    (a.0.min(b.0).min(c.0).floor().max(0.0) as usize, (a.0.max(b.0).max(c.0).ceil().max(0.0) as usize).min(sw));
                let (min_y, max_y) =
                    (a.1.min(b.1).min(c.1).floor().max(0.0) as usize, (a.1.max(b.1).max(c.1).ceil().max(0.0) as usize).min(sh));
                let (iz0, iz1, iz2) = (1.0 / cam[i0][2], 1.0 / cam[i1][2], 1.0 / cam[i2][2]);
                for y in min_y..max_y {
                    for x in min_x..max_x {
                        let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                        let w0 = ((b.0 - px) * (c.1 - py) - (b.1 - py) * (c.0 - px)) / area;
                        let w1 = ((c.0 - px) * (a.1 - py) - (c.1 - py) * (a.0 - px)) / area;
                        let w2 = 1.0 - w0 - w1;
                        if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                            continue;
                        }
                        // 遠近補正つきの重心座標
                        let iz = w0 * iz0 + w1 * iz1 + w2 * iz2;
                        let z = 1.0 / iz;
                        let idx = y * sw + x;
                        if z as f32 >= depth[idx] {
                            continue;
                        }
                        let (p0, p1, p2) = (w0 * iz0 / iz, w1 * iz1 / iz, w2 * iz2 / iz);
                        let pw = add(add(mul(world[i0], p0), mul(world[i1], p1)), mul(world[i2], p2));
                        let n = norm(add(add(mul(normals[i0], p0), mul(normals[i1], p1)), mul(normals[i2], p2)));
                        let uv = match mesh.uvs.as_ref() {
                            Some(t) => [t[i0][0] * p0 + t[i1][0] * p1 + t[i2][0] * p2, t[i0][1] * p0 + t[i1][1] * p1 + t[i2][1] * p2],
                            None => [0.0, 0.0],
                        };
                        let view = norm(sub(self.camera.pos, pw));
                        let rgb = shade_pbr(&mesh.material, uv, pw, n, view, lights, ambient);
                        depth[idx] = z as f32;
                        color[idx * 4..idx * 4 + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 1.0]);
                    }
                }
            }
        }
        // 縮小（色は平均、深度は最も手前）
        let mut out_c = vec![0.0f32; ow * oh * 4];
        let mut out_d = vec![f32::INFINITY; ow * oh];
        for y in 0..oh {
            for x in 0..ow {
                let mut acc = [0.0f32; 4];
                let mut z = f32::INFINITY;
                for j in 0..SS {
                    for i in 0..SS {
                        let idx = (y * SS + j) * sw + x * SS + i;
                        (0..4).for_each(|k| acc[k] += color[idx * 4 + k]);
                        z = z.min(depth[idx]);
                    }
                }
                let n = (SS * SS) as f32;
                out_c[(y * ow + x) * 4..(y * ow + x) * 4 + 4].copy_from_slice(&acc.map(|v| v / n * opacity));
                out_d[y * ow + x] = z;
            }
        }
        let center_depth = self.camera.view(xf.point([0.0; 3]))[2];
        let mut r = Rendered { color: out_c, depth: out_d, center_depth, blend: layer.blend_mode };
        self.depth_of_field(&mut r);
        Some(r)
    }

    /// 3D レイヤーのまとまりを描いて `canvas` に合成する。
    pub fn render(&self, canvas: &mut FrameBuf, comp: &Composition, items: &[SceneItem], t: Time, base_dir: Option<&Path>) {
        // 影を落とすレイヤー（平面のみ）
        let casters: Vec<Caster> = items
            .iter()
            .filter_map(|it| match it {
                SceneItem::Plane { layer, buf, width, height, origin, opacity } if layer.casts_shadows => Some(Caster {
                    id: layer.id.clone(),
                    xf: world_xf(comp, layer, t),
                    buf: buf.clone(),
                    width: *width,
                    height: *height,
                    origin: *origin,
                    opacity: *opacity,
                }),
                _ => None,
            })
            .collect();

        let mut rendered: Vec<Rendered> = Vec::new();
        let mut has_model = false;
        for item in items {
            let r = match item {
                SceneItem::Plane { layer, buf, width, height, opacity, origin } => {
                    self.render_plane(layer, buf, *width, *height, *origin, *opacity, &world_xf(comp, layer, t), &casters)
                }
                SceneItem::Model { layer, path, opacity } => {
                    has_model = true;
                    let full = match base_dir {
                        Some(d) if Path::new(path).is_relative() => d.join(path),
                        _ => Path::new(path).to_path_buf(),
                    };
                    match load_model_at(&full, znd_core::eval::layer_source_seconds(comp, layer, t)) {
                        Ok(model) => {
                            // モデルは「最大寸法がコンポの高さの半分」になるよう正規化して置く
                            let fit = comp.height as f64 * 0.5 / model.extent.max(1e-9);
                            let c = model.center;
                            let normalize = Xf::scale([fit, -fit, fit]).then_after(&Xf::translate([-c[0], -c[1], -c[2]]));
                            self.render_model(layer, &model, *opacity, &world_xf(comp, layer, t).then_after(&normalize))
                        }
                        Err(e) => {
                            eprintln!("3D モデル: {e}");
                            None
                        }
                    }
                }
            };
            rendered.extend(r);
        }
        // 奥から手前へ
        rendered.sort_by(|a, b| b.center_depth.total_cmp(&a.center_depth));

        if !has_model {
            // 平面だけなら、距離順に各レイヤーの描画モードでそのまま合成する
            for r in &rendered {
                for (d, s) in canvas.data.chunks_exact_mut(4).zip(r.color.chunks_exact(4)) {
                    if s[3] > 0.0 || s[0] > 0.0 {
                        let o = blend_pixel([d[0], d[1], d[2], d[3]], [s[0], s[1], s[2], s[3]], r.blend);
                        d.copy_from_slice(&o);
                    }
                }
            }
            return;
        }

        // モデルを含む場合は画素ごとの深度で前後を決める（モデルと平面が交差しても正しく隠れる）
        let n = canvas.width * canvas.height;
        let mut bin = vec![0.0f32; n * 4];
        let mut zbuf = vec![f32::INFINITY; n];
        for r in &rendered {
            for i in 0..n {
                let s = &r.color[i * 4..i * 4 + 4];
                if s[3] <= 0.0 {
                    continue;
                }
                let d = &mut bin[i * 4..i * 4 + 4];
                if r.depth[i] <= zbuf[i] {
                    let k = 1.0 - s[3];
                    (0..4).for_each(|c| d[c] = s[c] + d[c] * k);
                    if s[3] > 0.5 {
                        zbuf[i] = r.depth[i];
                    }
                } else {
                    // 既にある絵より奥: 後ろ側に合成する
                    let k = 1.0 - d[3];
                    (0..4).for_each(|c| d[c] += s[c] * k);
                }
            }
        }
        for (d, s) in canvas.data.chunks_exact_mut(4).zip(bin.chunks_exact(4)) {
            let k = 1.0 - s[3];
            (0..4).for_each(|c| d[c] = s[c] + d[c] * k);
        }
    }
}

struct Caster {
    id: znd_core::LayerId,
    xf: Xf,
    buf: Arc<FrameBuf>,
    width: f64,
    height: f64,
    origin: [f64; 2],
    opacity: f32,
}

/// 点 `p` から光源へ向かう線分が、影を落とすレイヤーに遮られる度合い（1 = 影なし）。
fn shadow_factor(light: &Light, p: V3, receiver: &znd_core::LayerId, casters: &[Caster]) -> f32 {
    let (dir, max_t) = match light.kind {
        LightType::Parallel => (mul(light.dir, -1.0), f64::MAX),
        _ => {
            let d = sub(light.pos, p);
            (norm(d), dot(d, d).sqrt())
        }
    };
    let mut lit = 1.0f32;
    for c in casters {
        if &c.id == receiver {
            continue;
        }
        // 光線 p + s·dir と平面の交点
        let o = c.xf.point([c.origin[0], c.origin[1], 0.0]);
        let (u, v) = (c.xf.dir([1.0, 0.0, 0.0]), c.xf.dir([0.0, 1.0, 0.0]));
        let n = cross(u, v);
        let denom = dot(dir, n);
        if denom.abs() < 1e-12 {
            continue;
        }
        let s = dot(sub(o, p), n) / denom;
        if s <= 1e-3 || s >= max_t {
            continue;
        }
        let hit = sub(add(p, mul(dir, s)), o);
        let (uu, uv, vv) = (dot(u, u), dot(u, v), dot(v, v));
        let det = uu * vv - uv * uv;
        if det.abs() < 1e-18 {
            continue;
        }
        let (hu, hv) = (dot(hit, u), dot(hit, v));
        let (a, b) = ((hu * vv - hv * uv) / det, (hv * uu - hu * uv) / det);
        if a < 0.0 || b < 0.0 || a > c.width || b > c.height {
            continue;
        }
        let alpha =
            crate::effects::sample_clamped(&c.buf, (a / c.width * c.buf.width as f64) as f32, (b / c.height * c.buf.height as f64) as f32)
                [3]
                * c.opacity;
        lit *= 1.0 - alpha * light.shadow_darkness;
    }
    lit
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: V3, b: V3) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-9)
    }

    #[test]
    fn rotations_follow_documented_directions() {
        // Z 回転は 2D と同じ（+X が +Y へ）
        assert!(close(Xf::rot_z(90.0).dir([1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]));
        // Y 回転: 右端（+X）が奥（+Z）へ
        assert!(close(Xf::rot_y(90.0).dir([1.0, 0.0, 0.0]), [0.0, 0.0, 1.0]));
        // X 回転: 上端（-Y）が奥（+Z）へ
        assert!(close(Xf::rot_x(90.0).dir([0.0, -1.0, 0.0]), [0.0, 0.0, 1.0]));
        let m = Xf::translate([1.0, 2.0, 3.0]).then_after(&Xf::scale([2.0, 2.0, 2.0]));
        assert!(close(m.point([1.0, 1.0, 1.0]), [3.0, 4.0, 5.0]));
    }

    #[test]
    fn default_camera_projects_z0_plane_at_identity() {
        let comp = Composition::new("c", "c", 1920, 1080, znd_core::FrameRate::FPS_30, Time::from_seconds(1.0));
        let s = Scene::new(&comp, Time::ZERO, Affine::IDENTITY, 1920, 1080, &ColorXf::SRGB);
        for p in [[0.0, 0.0, 0.0], [1920.0, 1080.0, 0.0], [300.0, 700.0, 0.0]] {
            let (x, y) = s.project(s.camera.view(p));
            assert!((x - p[0]).abs() < 1e-6 && (y - p[1]).abs() < 1e-6);
        }
        // 奥（Z = ズーム値）にある点は、中心からの距離が半分に見える
        let z = default_zoom(&comp);
        let (x, _) = s.project(s.camera.view([960.0 + 400.0, 540.0, z]));
        assert!((x - 1160.0).abs() < 1e-6);
    }
}
