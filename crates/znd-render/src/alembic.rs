//! Alembic（Ogawa 形式）の読み込み。メッシュ（PolyMesh）、変換（Xform）、カメラに対応する。
//!
//! HDF5 形式の古い Alembic、細分割曲面・曲線・点群・NURBS は対象外。
//! 時間サンプルを持つファイルは、指定時刻の前後のサンプルを線形補間する（頂点数が変わる場合は直前のサンプル）。

use crate::model::{finish, Material, Mesh, Model};
use crate::three_d::V3;
use std::path::Path;

/// 行ベクトル規約の 4x4 行列（Imath と同じ並び。平行移動は m[12..15]）。
pub type M44 = [f64; 16];

const IDENTITY: M44 = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// `a` を先に、`b` を後に適用する行列（行ベクトル規約の a * b）。
fn mat_mul(a: &M44, b: &M44) -> M44 {
    let mut o = [0.0; 16];
    for r in 0..4 {
        for c in 0..4 {
            o[r * 4 + c] = (0..4).map(|k| a[r * 4 + k] * b[k * 4 + c]).sum();
        }
    }
    o
}

pub fn transform_point(m: &M44, p: V3) -> V3 {
    [
        p[0] * m[0] + p[1] * m[4] + p[2] * m[8] + m[12],
        p[0] * m[1] + p[1] * m[5] + p[2] * m[9] + m[13],
        p[0] * m[2] + p[1] * m[6] + p[2] * m[10] + m[14],
    ]
}

fn transform_dir(m: &M44, p: V3) -> V3 {
    [p[0] * m[0] + p[1] * m[4] + p[2] * m[8], p[0] * m[1] + p[1] * m[5] + p[2] * m[9], p[0] * m[2] + p[1] * m[6] + p[2] * m[10]]
}

#[derive(Debug, Clone)]
struct TimeSampling {
    time_per_cycle: f64,
    times: Vec<f64>,
}

impl TimeSampling {
    fn time(&self, i: usize) -> f64 {
        let n = self.times.len().max(1);
        if self.times.is_empty() {
            return 0.0;
        }
        // 非周期（各サンプルの時刻を全部持つ）は time_per_cycle が巨大な値になっている
        if self.time_per_cycle > 1e300 {
            return self.times[i.min(n - 1)];
        }
        self.times[i % n] + (i / n) as f64 * self.time_per_cycle
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PropKind {
    Compound,
    Scalar,
    Array,
}

#[derive(Debug, Clone)]
struct Prop {
    name: String,
    kind: PropKind,
    pod: u8,
    #[allow(dead_code)]
    extent: usize,
    samples: usize,
    first_changed: usize,
    last_changed: usize,
    ts: usize,
    meta: String,
    /// このプロパティのグループの位置。
    pos: u64,
}

#[derive(Debug, Clone)]
struct Object {
    name: String,
    meta: String,
    pos: u64,
}

pub struct Archive {
    data: Vec<u8>,
    root: u64,
    samplings: Vec<TimeSampling>,
    indexed_meta: Vec<String>,
}

const DATA_BIT: u64 = 1 << 63;

struct Cursor<'a> {
    d: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(n).filter(|e| *e <= self.d.len()).ok_or("Alembic: データが途中で切れています")?;
        let s = &self.d[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn f64(&mut self) -> Result<f64, String> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    /// サイズヒントに応じた幅の整数（0 = 1 バイト、1 = 2 バイト、2 = 4 バイト）。
    fn hinted(&mut self, hint: u32) -> Result<usize, String> {
        Ok(match hint {
            0 => self.take(1)?[0] as usize,
            1 => u16::from_le_bytes(self.take(2)?.try_into().unwrap()) as usize,
            _ => self.u32()? as usize,
        })
    }
    fn string(&mut self, n: usize) -> Result<String, String> {
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
}

impl Archive {
    pub fn open(path: &Path) -> Result<Archive, String> {
        let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_bytes(data).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn from_bytes(data: Vec<u8>) -> Result<Archive, String> {
        if data.len() >= 8 && &data[..8] == b"\x89HDF\r\n\x1a\n" {
            return Err("HDF5 形式の Alembic は未対応です。Ogawa 形式で書き出し直してください".into());
        }
        if data.len() < 16 || &data[..5] != b"Ogawa" {
            return Err("Alembic（Ogawa 形式）ではありません".into());
        }
        if data[5] != 0xff {
            return Err("Alembic: 書き込みが完了していないファイルです".into());
        }
        let root = u64::from_le_bytes(data[8..16].try_into().unwrap());
        let mut a = Archive { data, root, samplings: Vec::new(), indexed_meta: vec![String::new()] };
        let top = a.group(root)?;
        if top.len() < 6 {
            return Err("Alembic: 構造が不正です".into());
        }
        // 時間サンプリング
        let mut c = Cursor { d: a.bytes(top[4])?, pos: 0 };
        let mut samplings = Vec::new();
        while c.pos + 16 <= c.d.len() {
            let _max_sample = c.u32()?;
            let time_per_cycle = c.f64()?;
            let n = c.u32()? as usize;
            let mut times = Vec::with_capacity(n);
            for _ in 0..n {
                times.push(c.f64()?);
            }
            samplings.push(TimeSampling { time_per_cycle, times });
        }
        // 共有メタデータ
        let mut c = Cursor { d: a.bytes(top[5])?, pos: 0 };
        let mut meta = vec![String::new()];
        while c.pos < c.d.len() {
            let n = c.take(1)?[0] as usize;
            meta.push(c.string(n)?);
        }
        a.samplings = samplings;
        a.indexed_meta = meta;
        Ok(a)
    }

    /// グループの子（データなら最上位ビットが立っている）。
    fn group(&self, pos: u64) -> Result<Vec<u64>, String> {
        let pos = (pos & !DATA_BIT) as usize;
        if pos == 0 {
            return Ok(Vec::new());
        }
        let mut c = Cursor { d: &self.data, pos };
        let n = u64::from_le_bytes(c.take(8)?.try_into().unwrap()) as usize;
        if n > self.data.len() / 8 {
            return Err("Alembic: 構造が不正です".into());
        }
        let raw = c.take(n * 8)?;
        Ok(raw.chunks_exact(8).map(|b| u64::from_le_bytes(b.try_into().unwrap())).collect())
    }

    fn bytes(&self, child: u64) -> Result<&[u8], String> {
        if child & DATA_BIT == 0 {
            return Err("Alembic: データが必要な位置にグループがあります".into());
        }
        let pos = (child & !DATA_BIT) as usize;
        if pos == 0 {
            return Ok(&[]);
        }
        let mut c = Cursor { d: &self.data, pos };
        let n = u64::from_le_bytes(c.take(8)?.try_into().unwrap()) as usize;
        c.take(n)
    }

    fn children(&self, obj_pos: u64) -> Result<Vec<Object>, String> {
        let g = self.group(obj_pos)?;
        let Some(&last) = g.last() else { return Ok(Vec::new()) };
        if g.len() < 2 || last & DATA_BIT == 0 {
            return Ok(Vec::new());
        }
        let d = self.bytes(last)?;
        // 末尾 32 バイトはハッシュ
        let end = d.len().saturating_sub(32);
        let mut c = Cursor { d: &d[..end], pos: 0 };
        let mut out = Vec::new();
        let mut i = 1;
        while c.pos < end && i < g.len() - 1 {
            let n = c.u32()? as usize;
            let name = c.string(n)?;
            let mi = c.take(1)?[0];
            let meta = if mi == 0xff {
                let n = c.u32()? as usize;
                c.string(n)?
            } else {
                self.indexed_meta.get(mi as usize).cloned().unwrap_or_default()
            };
            out.push(Object { name, meta, pos: g[i] });
            i += 1;
        }
        Ok(out)
    }

    /// 複合プロパティ（グループ位置 `pos`）の中のプロパティ一覧。
    fn props(&self, pos: u64) -> Result<Vec<Prop>, String> {
        let g = self.group(pos)?;
        let Some(&last) = g.last() else { return Ok(Vec::new()) };
        if last & DATA_BIT == 0 {
            return Ok(Vec::new());
        }
        let d = self.bytes(last)?;
        let mut c = Cursor { d, pos: 0 };
        let mut out = Vec::new();
        let mut i = 0;
        while c.pos < d.len() && i < g.len() - 1 {
            let info = c.u32()?;
            let hint = (info >> 2) & 3;
            let ptype = info & 3;
            let mut p = Prop {
                name: String::new(),
                kind: match ptype {
                    0 => PropKind::Compound,
                    1 => PropKind::Scalar,
                    _ => PropKind::Array,
                },
                pod: ((info >> 4) & 0xf) as u8,
                extent: ((info >> 12) & 0xff) as usize,
                samples: 0,
                first_changed: 0,
                last_changed: 0,
                ts: 0,
                meta: String::new(),
                pos: g[i],
            };
            if ptype != 0 {
                p.samples = c.hinted(hint)?;
                if info & 0x200 != 0 {
                    p.first_changed = c.hinted(hint)?;
                    p.last_changed = c.hinted(hint)?;
                } else if info & 0x800 != 0 {
                    (p.first_changed, p.last_changed) = (0, 0);
                } else {
                    (p.first_changed, p.last_changed) = (1, p.samples.saturating_sub(1));
                }
                if info & 0x100 != 0 {
                    p.ts = c.hinted(hint)?;
                }
            }
            let n = c.hinted(hint)?;
            p.name = c.string(n)?;
            let mi = (info >> 20) & 0xff;
            p.meta = if mi == 0xff {
                let n = c.hinted(hint)?;
                c.string(n)?
            } else {
                self.indexed_meta.get(mi as usize).cloned().unwrap_or_default()
            };
            out.push(p);
            i += 1;
        }
        Ok(out)
    }

    fn find<'p>(props: &'p [Prop], name: &str) -> Option<&'p Prop> {
        props.iter().find(|p| p.name == name)
    }

    /// サンプル番号 → グループ内の位置。値が変わらない区間は同じデータを指す。
    fn stored_index(p: &Prop, i: usize) -> usize {
        if i < p.first_changed || (p.first_changed == 0 && p.last_changed == 0) {
            0
        } else if i >= p.last_changed {
            p.last_changed - p.first_changed + 1
        } else {
            i - p.first_changed + 1
        }
    }

    /// サンプルの生バイト列（先頭 16 バイトのハッシュを除く）。
    fn sample(&self, p: &Prop, i: usize) -> Result<&[u8], String> {
        let g = self.group(p.pos)?;
        let idx = Self::stored_index(p, i.min(p.samples.saturating_sub(1)));
        let child = match p.kind {
            PropKind::Scalar => g.get(idx),
            PropKind::Array => g.get(idx * 2),
            PropKind::Compound => None,
        };
        let d = self.bytes(*child.ok_or("Alembic: サンプルがありません")?)?;
        Ok(d.get(16..).unwrap_or(&[]))
    }

    fn f64s(&self, p: &Prop, i: usize) -> Result<Vec<f64>, String> {
        let d = self.sample(p, i)?;
        Ok(match p.pod {
            // float32
            10 => d.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap()) as f64).collect(),
            // float64
            11 => d.chunks_exact(8).map(|b| f64::from_le_bytes(b.try_into().unwrap())).collect(),
            _ => Vec::new(),
        })
    }

    fn ints(&self, p: &Prop, i: usize) -> Result<Vec<i64>, String> {
        let d = self.sample(p, i)?;
        Ok(match p.pod {
            0 | 1 | 2 => d.iter().map(|b| *b as i64).collect(),
            5 => d.chunks_exact(4).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as i64).collect(),
            6 => d.chunks_exact(4).map(|b| i32::from_le_bytes(b.try_into().unwrap()) as i64).collect(),
            _ => Vec::new(),
        })
    }

    /// 時刻 `seconds` に対する (前のサンプル番号, 次のサンプル番号, 補間係数)。
    fn locate(&self, p: &Prop, seconds: f64) -> (usize, usize, f64) {
        let n = p.samples;
        if n <= 1 {
            return (0, 0, 0.0);
        }
        let Some(ts) = self.samplings.get(p.ts) else { return (0, 0, 0.0) };
        if seconds <= ts.time(0) {
            return (0, 0, 0.0);
        }
        if seconds >= ts.time(n - 1) {
            return (n - 1, n - 1, 0.0);
        }
        // 二分探索
        let (mut lo, mut hi) = (0, n - 1);
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if ts.time(mid) <= seconds {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let (a, b) = (ts.time(lo), ts.time(hi));
        (lo, hi, if b > a { (seconds - a) / (b - a) } else { 0.0 })
    }

    fn lerp_f64s(&self, p: &Prop, seconds: f64) -> Result<Vec<f64>, String> {
        let (i0, i1, f) = self.locate(p, seconds);
        let a = self.f64s(p, i0)?;
        if i1 == i0 || f <= 0.0 {
            return Ok(a);
        }
        let b = self.f64s(p, i1)?;
        if a.len() != b.len() {
            return Ok(a);
        }
        Ok(a.iter().zip(&b).map(|(x, y)| x + (y - x) * f).collect())
    }

    fn prop_range(&self, p: &Prop, lo: &mut f64, hi: &mut f64, animated: &mut bool) {
        if p.samples > 1 && !(p.first_changed == 0 && p.last_changed == 0) {
            if let Some(ts) = self.samplings.get(p.ts) {
                *animated = true;
                *lo = lo.min(ts.time(0));
                *hi = hi.max(ts.time(p.samples - 1));
            }
        }
    }

    /// 変換（Xform）のローカル行列と、親の変換を継承するか。
    fn xform(&self, props: &[Prop], seconds: f64) -> Result<(M44, bool), String> {
        let Some(x) = Self::find(props, ".xform") else { return Ok((IDENTITY, true)) };
        let xp = self.props(x.pos)?;
        let inherits = match Self::find(&xp, ".inherits") {
            Some(p) => self.sample(p, self.locate(p, seconds).0)?.first().copied().unwrap_or(1) != 0,
            None => true,
        };
        let (Some(ops), Some(vals)) = (Self::find(&xp, ".ops"), Self::find(&xp, ".vals")) else { return Ok((IDENTITY, inherits)) };
        let ops = self.sample(ops, 0)?.to_vec();
        let vals = self.lerp_f64s(vals, seconds)?;
        let mut m = IDENTITY;
        let mut v = vals.iter().copied();
        let mut next = || v.next().unwrap_or(0.0);
        for op in ops {
            let local: M44 = match op >> 4 {
                0 => {
                    let (x, y, z) = (next(), next(), next());
                    [x, 0.0, 0.0, 0.0, 0.0, y, 0.0, 0.0, 0.0, 0.0, z, 0.0, 0.0, 0.0, 0.0, 1.0]
                }
                1 => {
                    let (x, y, z) = (next(), next(), next());
                    [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, x, y, z, 1.0]
                }
                2 => {
                    let axis = [next(), next(), next()];
                    rotation(axis, next())
                }
                3 => {
                    let mut o = [0.0; 16];
                    o.iter_mut().for_each(|e| *e = next());
                    o
                }
                4 => rotation([1.0, 0.0, 0.0], next()),
                5 => rotation([0.0, 1.0, 0.0], next()),
                6 => rotation([0.0, 0.0, 1.0], next()),
                _ => IDENTITY,
            };
            // 先に書かれた演算ほど外側（最後に適用）
            m = mat_mul(&local, &m);
        }
        Ok((m, inherits))
    }

    fn walk(&self, obj: &Object, parent: &M44, seconds: f64, out: &mut Scene, depth: usize) -> Result<(), String> {
        if depth > 256 {
            return Err("Alembic: 階層が深すぎます".into());
        }
        let g = self.group(obj.pos)?;
        let props = match g.first() {
            Some(&p) if p & DATA_BIT == 0 => self.props(p)?,
            _ => Vec::new(),
        };
        let mut world = *parent;
        if obj.meta.contains("AbcGeom_Xform") || Self::find(&props, ".xform").is_some() {
            let (local, inherits) = self.xform(&props, seconds)?;
            world = if inherits { mat_mul(&local, parent) } else { local };
            if let Some(x) = Self::find(&props, ".xform") {
                for p in self.props(x.pos)? {
                    self.prop_range(&p, &mut out.start, &mut out.end, &mut out.animated);
                }
            }
        }
        if let Some(geom) = Self::find(&props, ".geom") {
            let gp = self.props(geom.pos)?;
            if let (Some(p), Some(fi), Some(fc)) = (Self::find(&gp, "P"), Self::find(&gp, ".faceIndices"), Self::find(&gp, ".faceCounts")) {
                self.prop_range(p, &mut out.start, &mut out.end, &mut out.animated);
                if obj.meta.contains("SubD") {
                    out.notes.push(format!("「{}」: 細分割曲面は元の多角形のまま読み込みました", obj.name));
                }
                out.meshes.push(self.mesh(&gp, p, fi, fc, &world, seconds)?);
            } else if let Some(core) = Self::find(&gp, ".core") {
                self.prop_range(core, &mut out.start, &mut out.end, &mut out.animated);
                let c = self.lerp_f64s(core, seconds)?;
                if c.len() >= 16 {
                    out.cameras.push(Camera {
                        name: obj.name.clone(),
                        world,
                        focal_length_mm: c[0],
                        horizontal_aperture_mm: c[1] * 10.0,
                        vertical_aperture_mm: c[3] * 10.0,
                        near: c[14],
                        far: c[15],
                    });
                }
            } else if Self::find(&gp, "P").is_some() {
                out.notes.push(format!("「{}」: 曲線・点群は読み込んでいません", obj.name));
            }
        }
        for child in self.children(obj.pos)? {
            self.walk(&child, &world, seconds, out, depth + 1)?;
        }
        Ok(())
    }

    fn mesh(&self, gp: &[Prop], p: &Prop, fi: &Prop, fc: &Prop, world: &M44, seconds: f64) -> Result<Mesh, String> {
        let pts = self.lerp_f64s(p, seconds)?;
        let points: Vec<V3> = pts.chunks_exact(3).map(|c| transform_point(world, [c[0], c[1], c[2]])).collect();
        let (i0, _, _) = self.locate(fi, seconds);
        let indices = self.ints(fi, i0)?;
        let counts = self.ints(fc, self.locate(fc, seconds).0)?;

        // 面頂点ごとの値（UV・法線）を、(値の配列, 添字) の形で読む
        let param = |name: &str, width: usize| -> Result<Option<(Vec<f64>, Option<Vec<i64>>, String)>, String> {
            let Some(pr) = Self::find(gp, name) else { return Ok(None) };
            match pr.kind {
                PropKind::Compound => {
                    let sub = self.props(pr.pos)?;
                    let Some(vals) = Self::find(&sub, ".vals") else { return Ok(None) };
                    let idx = match Self::find(&sub, ".indices") {
                        Some(ix) => Some(self.ints(ix, self.locate(ix, seconds).0)?),
                        None => None,
                    };
                    let v = self.f64s(vals, self.locate(vals, seconds).0)?;
                    Ok((v.len() % width == 0).then_some((v, idx, pr.meta.clone())))
                }
                PropKind::Array => {
                    let v = self.f64s(pr, self.locate(pr, seconds).0)?;
                    Ok((v.len() % width == 0).then_some((v, None, pr.meta.clone())))
                }
                PropKind::Scalar => Ok(None),
            }
        };
        let uv = param("uv", 2)?;
        let nrm = param("N", 3)?;
        let total: usize = counts.iter().map(|c| (*c).max(0) as usize).sum();
        // 値の個数から、面頂点ごとか頂点ごとかを判断する
        let lookup = |vals: &[f64], idx: &Option<Vec<i64>>, width: usize, corner: usize, vertex: usize| -> Option<Vec<f64>> {
            let n = idx.as_ref().map(|i| i.len()).unwrap_or(vals.len() / width);
            let slot = if n == total {
                corner
            } else if n == points.len() {
                vertex
            } else {
                return None;
            };
            let at = match idx {
                Some(ix) => *ix.get(slot)? as usize,
                None => slot,
            };
            vals.get(at * width..at * width + width).map(|s| s.to_vec())
        };

        let mut mesh = Mesh {
            positions: Vec::new(),
            normals: Vec::new(),
            uvs: uv.as_ref().map(|_| Vec::new()),
            indices: Vec::new(),
            material: Material::default(),
        };
        let mut corner = 0usize;
        let mut normals_ok = nrm.is_some();
        for &count in &counts {
            let count = count.max(0) as usize;
            let base = mesh.positions.len() as u32;
            for k in 0..count {
                let Some(&vi) = indices.get(corner + k) else { return Err("Alembic: 面の頂点番号が足りません".into()) };
                let vi = vi as usize;
                let Some(&pt) = points.get(vi) else { return Err("Alembic: 頂点番号が範囲外です".into()) };
                mesh.positions.push(pt);
                if let (Some((vals, idx, _)), Some(uvs)) = (&uv, &mut mesh.uvs) {
                    let v = lookup(vals, idx, 2, corner + k, vi).unwrap_or(vec![0.0, 0.0]);
                    // Alembic の UV は左下原点
                    uvs.push([v[0], 1.0 - v[1]]);
                }
                if let Some((vals, idx, _)) = &nrm {
                    match lookup(vals, idx, 3, corner + k, vi) {
                        Some(v) => mesh.normals.push(crate::three_d::norm(transform_dir(world, [v[0], v[1], v[2]]))),
                        None => normals_ok = false,
                    }
                }
            }
            // Alembic の面は時計回り。反時計回りに直しながら扇状に三角形分割する
            for k in 1..count.saturating_sub(1) {
                mesh.indices.extend([base, base + k as u32 + 1, base + k as u32]);
            }
            corner += count;
        }
        if !normals_ok {
            mesh.normals.clear();
        }
        Ok(mesh)
    }

    /// 時刻 `seconds` の状態を読む。
    pub fn scene_at(&self, seconds: f64) -> Result<Scene, String> {
        let mut out = Scene { meshes: Vec::new(), cameras: Vec::new(), start: f64::MAX, end: f64::MIN, animated: false, notes: Vec::new() };
        let top = self.group(self.root)?;
        let root = Object { name: "ABC".into(), meta: String::new(), pos: top[2] };
        self.walk(&root, &IDENTITY, seconds, &mut out, 0)?;
        if !out.animated {
            (out.start, out.end) = (0.0, 0.0);
        }
        Ok(out)
    }
}

fn rotation(axis: V3, degrees: f64) -> M44 {
    let len = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
    if len < 1e-12 {
        return IDENTITY;
    }
    let (x, y, z) = (axis[0] / len, axis[1] / len, axis[2] / len);
    let (s, c) = degrees.to_radians().sin_cos();
    let t = 1.0 - c;
    // 行ベクトル規約（列ベクトル規約の回転行列の転置）
    [
        t * x * x + c,
        t * x * y + s * z,
        t * x * z - s * y,
        0.0,
        t * x * y - s * z,
        t * y * y + c,
        t * y * z + s * x,
        0.0,
        t * x * z + s * y,
        t * y * z - s * x,
        t * z * z + c,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ]
}

#[derive(Debug, Clone)]
pub struct Camera {
    pub name: String,
    /// カメラ → ワールド。カメラは -Z を向き、+Y が上。
    pub world: M44,
    pub focal_length_mm: f64,
    pub horizontal_aperture_mm: f64,
    pub vertical_aperture_mm: f64,
    pub near: f64,
    pub far: f64,
}

impl Camera {
    pub fn position(&self) -> V3 {
        [self.world[12], self.world[13], self.world[14]]
    }
    /// 視線方向（単位ベクトル）。
    pub fn forward(&self) -> V3 {
        crate::three_d::norm(transform_dir(&self.world, [0.0, 0.0, -1.0]))
    }
    pub fn up(&self) -> V3 {
        crate::three_d::norm(transform_dir(&self.world, [0.0, 1.0, 0.0]))
    }
}

pub struct Scene {
    pub meshes: Vec<Mesh>,
    pub cameras: Vec<Camera>,
    /// アニメーションの時間範囲（秒）。静止していれば 0..0。
    pub start: f64,
    pub end: f64,
    pub animated: bool,
    pub notes: Vec<String>,
}

impl Scene {
    pub fn into_model(self) -> Result<Model, String> {
        finish(self.meshes)
    }
}
