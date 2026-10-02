//! 3D モデルの読み込み（glTF / GLB / OBJ / FBX / Alembic）と PBR シェーディング。

use crate::color::srgb_to_linear;
use crate::frame::FrameBuf;
use crate::three_d::{add, cross, dot, mul, norm, sub, Light, V3};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use znd_core::LightType;

#[derive(Debug, Clone)]
pub struct Material {
    /// リニアの基本色（RGBA）。
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    /// 基本色テクスチャ（リニア RGBA、ストレート）。
    pub texture: Option<Arc<FrameBuf>>,
}

impl Default for Material {
    fn default() -> Self {
        Material { base_color: [0.8, 0.8, 0.8, 1.0], metallic: 0.0, roughness: 0.6, texture: None }
    }
}

#[derive(Debug, Clone)]
pub struct Mesh {
    pub positions: Vec<V3>,
    pub normals: Vec<V3>,
    pub uvs: Option<Vec<[f64; 2]>>,
    pub indices: Vec<u32>,
    pub material: Material,
}

#[derive(Debug, Clone)]
pub struct Model {
    pub meshes: Vec<Mesh>,
    /// バウンディングボックスの中心と最大寸法（モデル座標）。
    pub center: V3,
    pub extent: f64,
}

pub(crate) fn finish(mut meshes: Vec<Mesh>) -> Result<Model, String> {
    meshes.retain(|m| !m.indices.is_empty());
    let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
    for p in meshes.iter().flat_map(|m| &m.positions) {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    if meshes.is_empty() || lo[0] > hi[0] {
        return Err("モデルに三角形がありません".into());
    }
    // 法線が無いメッシュは面法線から作る
    for m in &mut meshes {
        if m.normals.len() != m.positions.len() {
            let mut n = vec![[0.0; 3]; m.positions.len()];
            for tri in m.indices.chunks_exact(3) {
                let (a, b, c) = (m.positions[tri[0] as usize], m.positions[tri[1] as usize], m.positions[tri[2] as usize]);
                let face = cross(sub(b, a), sub(c, a));
                for i in tri {
                    n[*i as usize] = add(n[*i as usize], face);
                }
            }
            m.normals = n.into_iter().map(norm).collect();
        }
    }
    let center = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
    let extent = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(hi[2] - lo[2]);
    Ok(Model { meshes, center, extent })
}

/// Wavefront OBJ（と同じ場所の .mtl の Kd）を読む。
pub fn parse_obj(src: &str, mtl: Option<&str>) -> Result<Model, String> {
    let mut colors: HashMap<String, [f32; 4]> = HashMap::new();
    if let Some(mtl) = mtl {
        let mut name = String::new();
        for line in mtl.lines() {
            let mut it = line.split_whitespace();
            match it.next() {
                Some("newmtl") => name = it.next().unwrap_or("").to_string(),
                Some("Kd") => {
                    let v: Vec<f32> = it.filter_map(|s| s.parse().ok()).collect();
                    if v.len() >= 3 {
                        // MTL の色は表示値として書かれているのが通例
                        colors.insert(name.clone(), [srgb_to_linear(v[0]), srgb_to_linear(v[1]), srgb_to_linear(v[2]), 1.0]);
                    }
                }
                _ => {}
            }
        }
    }
    let (mut vs, mut vns, mut vts): (Vec<V3>, Vec<V3>, Vec<[f64; 2]>) = (Vec::new(), Vec::new(), Vec::new());
    let new_mesh =
        |material: Material| Mesh { positions: Vec::new(), normals: Vec::new(), uvs: Some(Vec::new()), indices: Vec::new(), material };
    let mut meshes = vec![new_mesh(Material::default())];
    let nums = |it: std::str::SplitWhitespace| -> Vec<f64> { it.filter_map(|s| s.parse().ok()).collect() };
    for (ln, line) in src.lines().enumerate() {
        let mut it = line.split_whitespace();
        match it.next() {
            Some("v") => {
                let v = nums(it);
                if v.len() < 3 {
                    return Err(format!("{} 行目: 頂点の座標が足りません", ln + 1));
                }
                vs.push([v[0], v[1], v[2]]);
            }
            Some("vn") => {
                let v = nums(it);
                if v.len() >= 3 {
                    vns.push([v[0], v[1], v[2]]);
                }
            }
            Some("vt") => {
                let v = nums(it);
                if v.len() >= 2 {
                    vts.push([v[0], 1.0 - v[1]]);
                }
            }
            Some("usemtl") => {
                let name = it.next().unwrap_or("");
                let base_color = colors.get(name).copied().unwrap_or(Material::default().base_color);
                meshes.push(new_mesh(Material { base_color, ..Material::default() }));
            }
            Some("f") => {
                let mesh = meshes.last_mut().unwrap();
                let mut corner = Vec::new();
                for tok in it {
                    let mut parts = tok.split('/');
                    // OBJ の番号は 1 始まり。負の値は末尾からの相対
                    let resolve = |s: Option<&str>, len: usize| -> Option<usize> {
                        let i: i64 = s?.parse().ok()?;
                        let idx = if i < 0 { len as i64 + i } else { i - 1 };
                        (idx >= 0 && (idx as usize) < len).then_some(idx as usize)
                    };
                    let Some(vi) = resolve(parts.next(), vs.len()) else {
                        return Err(format!("{} 行目: 頂点番号が範囲外です", ln + 1));
                    };
                    let ti = resolve(parts.next(), vts.len());
                    let ni = resolve(parts.next(), vns.len());
                    let idx = mesh.positions.len() as u32;
                    mesh.positions.push(vs[vi]);
                    mesh.uvs.as_mut().unwrap().push(ti.map(|i| vts[i]).unwrap_or([0.0, 0.0]));
                    if let Some(ni) = ni {
                        mesh.normals.push(vns[ni]);
                    }
                    corner.push(idx);
                }
                // 多角形は扇状に三角形分割する
                for k in 1..corner.len().saturating_sub(1) {
                    mesh.indices.extend([corner[0], corner[k], corner[k + 1]]);
                }
            }
            _ => {}
        }
    }
    finish(meshes)
}

fn mat_mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    // glTF の行列は列優先（m[列][行]）
    let mut o = [[0.0f32; 4]; 4];
    for c in 0..4 {
        for r in 0..4 {
            o[c][r] = (0..4).map(|k| a[k][r] * b[c][k]).sum();
        }
    }
    o
}

fn load_gltf(path: &Path) -> Result<Model, String> {
    let (doc, buffers, images) = gltf::import(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let textures: Vec<Option<Arc<FrameBuf>>> = images
        .iter()
        .map(|img| {
            let ch = match img.format {
                gltf::image::Format::R8G8B8 => 3,
                gltf::image::Format::R8G8B8A8 => 4,
                _ => return None,
            };
            let mut buf = FrameBuf::new(img.width as usize, img.height as usize);
            for (dst, src) in buf.data.chunks_exact_mut(4).zip(img.pixels.chunks_exact(ch)) {
                dst[0] = srgb_to_linear(src[0] as f32 / 255.0);
                dst[1] = srgb_to_linear(src[1] as f32 / 255.0);
                dst[2] = srgb_to_linear(src[2] as f32 / 255.0);
                dst[3] = if ch == 4 { src[3] as f32 / 255.0 } else { 1.0 };
            }
            Some(Arc::new(buf))
        })
        .collect();

    let mut meshes = Vec::new();
    const ID: [[f32; 4]; 4] = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
    let mut stack: Vec<(gltf::Node, [[f32; 4]; 4])> = Vec::new();
    for scene in doc.scenes() {
        stack.extend(scene.nodes().map(|n| (n, ID)));
    }
    while let Some((node, parent)) = stack.pop() {
        let m = mat_mul(&parent, &node.transform().matrix());
        stack.extend(node.children().map(|c| (c, m)));
        let Some(mesh) = node.mesh() else { continue };
        let point = |p: [f32; 3]| -> V3 {
            let f = |r: usize| (m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r]) as f64;
            [f(0), f(1), f(2)]
        };
        let dir = |p: [f32; 3]| -> V3 {
            let f = |r: usize| (m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2]) as f64;
            norm([f(0), f(1), f(2)])
        };
        for prim in mesh.primitives() {
            if prim.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let reader = prim.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
            let Some(pos) = reader.read_positions() else { continue };
            let positions: Vec<V3> = pos.map(point).collect();
            let normals: Vec<V3> = reader.read_normals().map(|n| n.map(dir).collect()).unwrap_or_default();
            let uvs: Option<Vec<[f64; 2]>> =
                reader.read_tex_coords(0).map(|t| t.into_f32().map(|uv| [uv[0] as f64, uv[1] as f64]).collect());
            let indices: Vec<u32> = match reader.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => (0..positions.len() as u32).collect(),
            };
            let pbr = prim.material().pbr_metallic_roughness();
            let material = Material {
                base_color: pbr.base_color_factor(),
                metallic: pbr.metallic_factor(),
                roughness: pbr.roughness_factor(),
                texture: pbr.base_color_texture().and_then(|t| textures.get(t.texture().source().index()).cloned().flatten()),
            };
            meshes.push(Mesh { positions, normals, uvs, indices, material });
        }
    }
    finish(meshes)
}

fn decode_texture(img: image::DynamicImage) -> Arc<FrameBuf> {
    let img = img.into_rgba8();
    let mut buf = FrameBuf::new(img.width() as usize, img.height() as usize);
    for (dst, src) in buf.data.chunks_exact_mut(4).zip(img.as_raw().chunks_exact(4)) {
        dst[0] = srgb_to_linear(src[0] as f32 / 255.0);
        dst[1] = srgb_to_linear(src[1] as f32 / 255.0);
        dst[2] = srgb_to_linear(src[2] as f32 / 255.0);
        dst[3] = src[3] as f32 / 255.0;
    }
    Arc::new(buf)
}

/// FBX（バイナリ・ASCII）を読む。座標系は右手系・Y 上に変換する。静止状態（アニメーションなし）のみ。
fn load_fbx(path: &Path) -> Result<Model, String> {
    let opts = ufbx::LoadOpts {
        target_axes: ufbx::CoordinateAxes::right_handed_y_up(),
        target_unit_meters: 1.0,
        space_conversion: ufbx::SpaceConversion::TransformRoot,
        generate_missing_normals: true,
        ..Default::default()
    };
    let scene = ufbx::load_file(&path.to_string_lossy(), opts)
        .map_err(|e| format!("{}: FBX を読み込めません: {}", path.display(), e.description))?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let material_of = |mat: &ufbx::Material| -> Material {
        let (pbr, fbx) = (&mat.pbr, &mat.fbx);
        let (map, factor) =
            if pbr.base_color.has_value { (&pbr.base_color, &pbr.base_factor) } else { (&fbx.diffuse_color, &fbx.diffuse_factor) };
        let k = if factor.has_value { factor.value_vec4.x as f32 } else { 1.0 };
        let c = map.value_vec4;
        // 仮定: FBX の色はリニア値として扱う（Blender は内部のリニア値をそのまま書き出す）
        let base_color = if map.has_value { [c.x as f32 * k, c.y as f32 * k, c.z as f32 * k, 1.0] } else { Material::default().base_color };
        let texture = map.texture.as_ref().and_then(|t| {
            let embedded: &[u8] = &t.content;
            if !embedded.is_empty() {
                return image::load_from_memory(embedded).ok().map(decode_texture);
            }
            let name: &str = &t.relative_filename;
            let candidates = [dir.join(name), Path::new(&*t.absolute_filename).to_path_buf(), Path::new(&*t.filename).to_path_buf()];
            candidates.iter().find(|p| p.is_file()).and_then(|p| image::open(p).ok()).map(decode_texture)
        });
        Material {
            base_color: if texture.is_some() { [k, k, k, 1.0] } else { base_color },
            metallic: if pbr.metalness.has_value { pbr.metalness.value_vec4.x as f32 } else { 0.0 },
            roughness: if pbr.roughness.has_value { pbr.roughness.value_vec4.x as f32 } else { 0.6 },
            texture,
        }
    };
    let mut meshes = Vec::new();
    for node in &scene.nodes {
        let Some(mesh) = &node.mesh else { continue };
        let m = &node.geometry_to_world;
        let mut tri = vec![0u32; mesh.max_face_triangles * 3];
        // マテリアルごとに分ける（マテリアルが無いメッシュは 1 つにまとめる）
        let parts = mesh.materials.len().max(1);
        let mut out: Vec<Mesh> = (0..parts)
            .map(|i| Mesh {
                positions: Vec::new(),
                normals: Vec::new(),
                uvs: mesh.vertex_uv.exists.then(Vec::new),
                indices: Vec::new(),
                material: mesh.materials.get(i).map(|mat| material_of(mat)).unwrap_or_default(),
            })
            .collect();
        for (fi, face) in mesh.faces.iter().enumerate() {
            let part = mesh.face_material.get(fi).map(|i| *i as usize).filter(|i| *i < parts).unwrap_or(0);
            let o = &mut out[part];
            let n = mesh.triangulate_face(&mut tri, *face) as usize;
            for &ix in &tri[..n * 3] {
                let ix = ix as usize;
                let p = ufbx::transform_position(m, mesh.vertex_position[ix]);
                o.indices.push(o.positions.len() as u32);
                o.positions.push([p.x, p.y, p.z]);
                if mesh.vertex_normal.exists {
                    let d = ufbx::transform_direction(m, mesh.vertex_normal[ix]);
                    o.normals.push(norm([d.x, d.y, d.z]));
                }
                if let Some(uvs) = &mut o.uvs {
                    let uv = mesh.vertex_uv[ix];
                    // FBX の UV は左下原点
                    uvs.push([uv.x, 1.0 - uv.y]);
                }
            }
        }
        meshes.extend(out);
    }
    finish(meshes)
}

/// 時刻つきでモデルを読み込む。アニメーションを持つ Alembic は `seconds` の状態を返し、
/// それ以外は `load_model` と同じ。中心と大きさは最初の状態のものを使う（動いても基準がぶれない）。
pub fn load_model_at(path: &Path, seconds: f64) -> Result<Arc<Model>, String> {
    let is_abc = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("abc"));
    let base = load_model(path)?;
    if !is_abc {
        return Ok(base);
    }
    type Key = (PathBuf, i64);
    static ARCHIVES: OnceLock<Mutex<HashMap<PathBuf, Arc<crate::alembic::Archive>>>> = OnceLock::new();
    static FRAMES: OnceLock<Mutex<HashMap<Key, Arc<Model>>>> = OnceLock::new();
    // 1/1000 秒単位でキャッシュする
    let key = (path.to_path_buf(), (seconds * 1000.0).round() as i64);
    let frames = FRAMES.get_or_init(Mutex::default);
    if let Some(hit) = frames.lock().unwrap().get(&key) {
        return Ok(hit.clone());
    }
    let archive = {
        let mut map = ARCHIVES.get_or_init(Mutex::default).lock().unwrap();
        match map.get(path) {
            Some(a) => a.clone(),
            None => {
                let a = Arc::new(crate::alembic::Archive::open(path)?);
                map.insert(path.to_path_buf(), a.clone());
                a
            }
        }
    };
    let scene = archive.scene_at(seconds)?;
    if !scene.animated {
        return Ok(base);
    }
    let mut model = scene.into_model()?;
    (model.center, model.extent) = (base.center, base.extent);
    let model = Arc::new(model);
    let mut f = frames.lock().unwrap();
    if f.len() > 64 {
        f.clear();
    }
    f.insert(key, model.clone());
    Ok(model)
}

/// モデルを読み込む（結果はキャッシュする）。
pub fn load_model(path: &Path) -> Result<Arc<Model>, String> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Result<Arc<Model>, String>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Mutex::default);
    if let Some(hit) = cache.lock().unwrap().get(path) {
        return hit.clone();
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let result = match ext.as_str() {
        "obj" => std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display())).and_then(|src| {
            let mtl = std::fs::read_to_string(path.with_extension("mtl")).ok();
            parse_obj(&src, mtl.as_deref())
        }),
        "gltf" | "glb" => load_gltf(path),
        "fbx" => load_fbx(path),
        "abc" => crate::alembic::Archive::open(path).and_then(|a| a.scene_at(f64::MIN)).and_then(|s| s.into_model()),
        other => Err(format!("未対応のモデル形式です: .{other}（glTF / GLB / OBJ / FBX / Alembic に対応）")),
    }
    .map(Arc::new);
    cache.lock().unwrap().insert(path.to_path_buf(), result.clone());
    result
}

fn texture_color(tex: &FrameBuf, uv: [f64; 2]) -> [f32; 4] {
    // 繰り返しラップ＋バイリニア
    let (u, v) = (uv[0].rem_euclid(1.0) * tex.width as f64 - 0.5, uv[1].rem_euclid(1.0) * tex.height as f64 - 0.5);
    let (x0, y0) = (u.floor(), v.floor());
    let (fx, fy) = ((u - x0) as f32, (v - y0) as f32);
    let at =
        |x: f64, y: f64| tex.pixel((x as i64).rem_euclid(tex.width as i64) as usize, (y as i64).rem_euclid(tex.height as i64) as usize);
    let (a, b, c, d) = (at(x0, y0), at(x0 + 1.0, y0), at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0));
    let mut o = [0.0; 4];
    for k in 0..4 {
        let top = a[k] + (b[k] - a[k]) * fx;
        let bot = c[k] + (d[k] - c[k]) * fx;
        o[k] = top + (bot - top) * fy;
    }
    o
}

/// メタリック・ラフネスの PBR（GGX）。`view` は表面からカメラへの単位ベクトル。
/// 強度 1 のライトが正面から当たった拡散面が、基本色そのものの明るさになるよう正規化している。
pub fn shade_pbr(mat: &Material, uv: [f64; 2], p: V3, n: V3, view: V3, lights: &[Light], ambient: f32) -> [f32; 3] {
    let tex = mat.texture.as_ref().map(|t| texture_color(t, uv)).unwrap_or([1.0; 4]);
    let albedo = [mat.base_color[0] * tex[0], mat.base_color[1] * tex[1], mat.base_color[2] * tex[2]];
    // 裏面も同じように照らす
    let n = if dot(n, view) < 0.0 { mul(n, -1.0) } else { n };
    let nv = dot(n, view).max(1e-4) as f32;
    let rough = mat.roughness.clamp(0.04, 1.0);
    let a2 = (rough * rough).powi(2);
    let k = (rough + 1.0).powi(2) / 8.0;
    let f0 = [0, 1, 2].map(|i| 0.04 + (albedo[i] - 0.04) * mat.metallic);
    let mut out = albedo.map(|c| c * ambient);
    for light in lights {
        if light.kind == LightType::Ambient {
            (0..3).for_each(|i| out[i] += albedo[i] * light.color[i]);
            continue;
        }
        let l = match light.kind {
            LightType::Parallel => mul(light.dir, -1.0),
            _ => norm(sub(light.pos, p)),
        };
        let nl = dot(n, l) as f32;
        if nl <= 0.0 {
            continue;
        }
        let mut atten = 1.0f32;
        if light.kind == LightType::Spot {
            let c = dot(mul(l, -1.0), light.dir);
            let (outer, inner) = light.cone_cos;
            atten = if inner > outer {
                ((c - outer) / (inner - outer)).clamp(0.0, 1.0) as f32
            } else if c >= outer {
                1.0
            } else {
                0.0
            };
        }
        let h = norm(add(l, view));
        let (nh, vh) = (dot(n, h).max(0.0) as f32, dot(view, h).max(0.0) as f32);
        let d = a2 / (std::f32::consts::PI * ((nh * nh) * (a2 - 1.0) + 1.0).powi(2));
        let g = (nl / (nl * (1.0 - k) + k)) * (nv / (nv * (1.0 - k) + k));
        let fres = (1.0 - vh).powi(5);
        for i in 0..3 {
            let f = f0[i] + (1.0 - f0[i]) * fres;
            let spec = d * g * f / (4.0 * nl * nv + 1e-4);
            let diffuse = (1.0 - f) * (1.0 - mat.metallic) * albedo[i] / std::f32::consts::PI;
            out[i] += (diffuse + spec) * light.color[i] * atten * nl * std::f32::consts::PI;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CUBE: &str = "\
v -1 -1 -1\nv 1 -1 -1\nv 1 1 -1\nv -1 1 -1\nv -1 -1 1\nv 1 -1 1\nv 1 1 1\nv -1 1 1\n\
usemtl red\n\
f 1 2 3 4\nf 5 8 7 6\nf 1 5 6 2\nf 2 6 7 3\nf 3 7 8 4\nf 5 1 4 8\n";

    #[test]
    fn obj_cube_parses_with_material_and_generated_normals() {
        let m = parse_obj(CUBE, Some("newmtl red\nKd 1.0 0.0 0.0\n")).unwrap();
        assert_eq!(m.meshes.len(), 1);
        let mesh = &m.meshes[0];
        assert_eq!(mesh.indices.len(), 36);
        assert_eq!(mesh.normals.len(), mesh.positions.len());
        assert_eq!(mesh.material.base_color, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(m.center, [0.0, 0.0, 0.0]);
        assert_eq!(m.extent, 2.0);
        assert!(parse_obj("v 0 0 0\nf 1 2 3\n", None).is_err());
        assert!(parse_obj("", None).is_err());
        // 負のインデックス
        assert!(parse_obj("v 0 0 0\nv 1 0 0\nv 0 1 0\nf -3 -2 -1\n", None).is_ok());
    }

    #[test]
    fn pbr_diffuse_matches_albedo_under_unit_frontal_light() {
        let mat = Material { base_color: [0.5, 0.25, 0.1, 1.0], metallic: 0.0, roughness: 1.0, texture: None };
        let light = Light {
            kind: LightType::Parallel,
            pos: [0.0; 3],
            dir: [0.0, 0.0, 1.0],
            color: [1.0; 3],
            cone_cos: (0.0, 0.0),
            casts_shadows: false,
            shadow_darkness: 0.0,
        };
        // 面はカメラ（-Z 側）を向き、光も正面から
        let c = shade_pbr(&mat, [0.0, 0.0], [0.0; 3], [0.0, 0.0, -1.0], [0.0, 0.0, -1.0], std::slice::from_ref(&light), 0.0);
        assert!((c[0] - 0.5).abs() < 0.06 && (c[1] - 0.25).abs() < 0.04, "{c:?}");
        // 斜め 60° なら半分
        let tilted = Light { dir: norm([0.0, 3f64.sqrt(), 1.0]), ..light };
        let c2 = shade_pbr(&mat, [0.0, 0.0], [0.0; 3], [0.0, 0.0, -1.0], [0.0, 0.0, -1.0], &[tilted], 0.0);
        assert!((c2[0] / c[0] - 0.5).abs() < 0.08, "{}", c2[0] / c[0]);
        // 光が裏から当たる面は暗い（環境光のみ）
        let back = Light {
            kind: LightType::Parallel,
            pos: [0.0; 3],
            dir: [0.0, 0.0, -1.0],
            color: [1.0; 3],
            cone_cos: (0.0, 0.0),
            casts_shadows: false,
            shadow_darkness: 0.0,
        };
        let c3 = shade_pbr(&mat, [0.0, 0.0], [0.0; 3], [0.0, 0.0, -1.0], [0.0, 0.0, -1.0], &[back], 0.2);
        assert!((c3[0] - 0.1).abs() < 1e-5);
    }
}
