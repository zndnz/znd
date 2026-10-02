//! PSD / PSB の読み込み。レイヤー構造を保ったままコンポにする。
//!
//! 対応: RGB・グレースケールの 8/16bit、ラスターレイヤー（テキストやスマートオブジェクトは
//! ファイルに保存されているラスター化済みの絵を使う）、グループ、描画モード、不透明度、表示状態、
//! レイヤーマスク（アルファに焼き込む）、クリッピング（下のレイヤーをアルファマットにする）。
//! 再現できない要素（調整レイヤー、レイヤー効果、ベクトルマスク等）は読み飛ばして `warnings` に列挙する。

use crate::MediaError;
use std::collections::BTreeSet;
use std::io::Read;
use std::path::Path;
use znd_core::{
    BlendMode, CompId, Composition, FootageId, FootageItem, FrameRate, Layer, LayerId, LayerKind, MatteMode, Project, Property, Rights,
    Time, TrackMatte, Transform,
};

struct Reader<'a> {
    d: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(n).filter(|e| *e <= self.d.len()).ok_or("ファイルが途中で切れています")?;
        let s = &self.d[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn i16(&mut self) -> Result<i16, String> {
        Ok(self.u16()? as i16)
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> Result<i32, String> {
        Ok(self.u32()? as i32)
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }
    /// PSD は 32bit、PSB は 64bit の長さ。
    fn len(&mut self, big: bool) -> Result<usize, String> {
        Ok(if big { self.u64()? as usize } else { self.u32()? as usize })
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Rect {
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
}

impl Rect {
    fn w(&self) -> usize {
        (self.right - self.left).max(0) as usize
    }
    fn h(&self) -> usize {
        (self.bottom - self.top).max(0) as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Section {
    None,
    /// グループの見出し（開閉どちらでも）。
    Folder,
    /// グループの終端（ファイル内ではグループの最初に現れる）。
    Divider,
}

struct Record {
    rect: Rect,
    channels: Vec<(i16, usize)>,
    blend: [u8; 4],
    opacity: u8,
    clipping: bool,
    hidden: bool,
    mask: Option<(Rect, u8, bool)>,
    name: String,
    section: Section,
    /// 再現できない付加情報のキー。
    extras: Vec<[u8; 4]>,
    /// チャンネル ID → 8bit または 16bit のサンプル（0..1 に正規化）。
    planes: Vec<(i16, Vec<f32>)>,
}

/// 読み込んだレイヤー 1 枚（下から上の順）。
#[derive(Debug, Clone)]
pub struct PsdLayer {
    pub name: String,
    pub left: i32,
    pub top: i32,
    pub width: usize,
    pub height: usize,
    /// ストレートアルファの RGBA（表示値 0..1）。マスク適用済み。
    pub rgba: Vec<f32>,
    pub opacity: f64,
    pub visible: bool,
    pub blend: BlendMode,
    pub clipping: bool,
    /// 所属グループ（外側から）。
    pub groups: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PsdDocument {
    pub width: u32,
    pub height: u32,
    pub depth: u16,
    pub layers: Vec<PsdLayer>,
    pub warnings: Vec<String>,
}

fn blend_of(key: &[u8; 4]) -> Option<BlendMode> {
    Some(match key {
        b"norm" | b"pass" => BlendMode::Normal,
        b"dark" => BlendMode::Darken,
        b"mul " => BlendMode::Multiply,
        b"idiv" => BlendMode::ColorBurn,
        b"lbrn" => BlendMode::LinearBurn,
        b"dkCl" => BlendMode::DarkerColor,
        b"lite" => BlendMode::Lighten,
        b"scrn" => BlendMode::Screen,
        b"div " => BlendMode::ColorDodge,
        b"lddg" => BlendMode::Add,
        b"lgCl" => BlendMode::LighterColor,
        b"over" => BlendMode::Overlay,
        b"sLit" => BlendMode::SoftLight,
        b"hLit" => BlendMode::HardLight,
        b"vLit" => BlendMode::VividLight,
        b"lLit" => BlendMode::LinearLight,
        b"pLit" => BlendMode::PinLight,
        b"hMix" => BlendMode::HardMix,
        b"diff" => BlendMode::Difference,
        b"smud" => BlendMode::Exclusion,
        b"fsub" => BlendMode::Subtract,
        b"fdiv" => BlendMode::Divide,
        b"hue " => BlendMode::Hue,
        b"sat " => BlendMode::Saturation,
        b"colr" => BlendMode::Color,
        b"lum " => BlendMode::Luminosity,
        _ => return None,
    })
}

fn extra_label(key: &[u8; 4]) -> Option<&'static str> {
    Some(match key {
        b"lfx2" | b"lrFX" | b"lmfx" => "レイヤー効果（ドロップシャドウ等）",
        b"vmsk" | b"vsms" => "ベクトルマスク",
        b"levl" | b"curv" | b"hue2" | b"hue " | b"brit" | b"blnc" | b"selc" | b"mixr" | b"grdm" | b"phfl" | b"expA" | b"vibA" | b"blwh"
        | b"nvrt" | b"post" | b"thrs" | b"clrL" => "調整レイヤー",
        b"SoCo" | b"GdFl" | b"PtFl" => "塗りつぶしレイヤー",
        _ => return None,
    })
}

fn unpack_bits(src: &[u8], out: &mut Vec<u8>, want: usize) {
    let start = out.len();
    let mut i = 0;
    while i < src.len() && out.len() - start < want {
        let n = src[i] as i8;
        i += 1;
        if n >= 0 {
            let len = (n as usize + 1).min(src.len() - i);
            out.extend_from_slice(&src[i..i + len]);
            i += len;
        } else if n != -128 {
            let Some(&v) = src.get(i) else { break };
            out.extend(std::iter::repeat(v).take((1 - n as isize) as usize));
            i += 1;
        }
    }
    out.resize(start + want, 0);
}

/// 1 チャンネルを読み、0..1 のサンプル列にする。`data` は圧縮方式（2 バイト）から始まる。
fn decode_channel(data: &[u8], w: usize, h: usize, depth: u16, big: bool) -> Result<Vec<f32>, String> {
    let bps = (depth / 8) as usize;
    let row_bytes = w * bps;
    let mut r = Reader { d: data, pos: 0 };
    let compression = r.u16()?;
    let mut raw: Vec<u8> = Vec::with_capacity(row_bytes * h);
    match compression {
        0 => raw.extend_from_slice(r.take((row_bytes * h).min(data.len() - 2))?),
        1 => {
            let mut counts = Vec::with_capacity(h);
            for _ in 0..h {
                counts.push(if big { r.u32()? as usize } else { r.u16()? as usize });
            }
            for c in counts {
                let row = r.take(c.min(r.d.len() - r.pos))?;
                unpack_bits(row, &mut raw, row_bytes);
            }
        }
        2 | 3 => {
            let mut z = flate2::read::ZlibDecoder::new(&data[2..]);
            z.read_to_end(&mut raw).map_err(|e| format!("ZIP 圧縮のチャンネルを展開できません: {e}"))?;
            raw.resize(row_bytes * h, 0);
            if compression == 3 {
                // 予測つき: 行ごとに、左隣との差分で記録されている
                for row in raw.chunks_exact_mut(row_bytes.max(1)) {
                    if bps == 1 {
                        for x in 1..w {
                            row[x] = row[x].wrapping_add(row[x - 1]);
                        }
                    } else if bps == 2 {
                        for x in 1..w {
                            let prev = u16::from_be_bytes([row[2 * x - 2], row[2 * x - 1]]);
                            let cur = u16::from_be_bytes([row[2 * x], row[2 * x + 1]]).wrapping_add(prev);
                            row[2 * x..2 * x + 2].copy_from_slice(&cur.to_be_bytes());
                        }
                    }
                }
            }
        }
        other => return Err(format!("未対応の圧縮方式です（{other}）")),
    }
    raw.resize(row_bytes * h, 0);
    Ok(match bps {
        1 => raw.iter().map(|v| *v as f32 / 255.0).collect(),
        2 => raw.chunks_exact(2).map(|v| u16::from_be_bytes([v[0], v[1]]) as f32 / 65535.0).collect(),
        _ => return Err(format!("{depth}bit の PSD は未対応です（8bit または 16bit で保存してください）")),
    })
}

fn pascal_padded(r: &mut Reader, pad: usize) -> Result<String, String> {
    let n = r.u8()? as usize;
    let bytes = r.take(n)?;
    let total = 1 + n;
    r.take((pad - total % pad) % pad)?;
    Ok(String::from_utf8_lossy(bytes).into_owned())
}

fn read_layer_info(r: &mut Reader, big: bool, depth: u16, color_channels: usize) -> Result<Vec<Record>, String> {
    let count = r.i16()?.unsigned_abs() as usize;
    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        let rect = Rect { top: r.i32()?, left: r.i32()?, bottom: r.i32()?, right: r.i32()? };
        let nch = r.u16()? as usize;
        let mut channels = Vec::with_capacity(nch);
        for _ in 0..nch {
            channels.push((r.i16()?, r.len(big)?));
        }
        if r.take(4)? != b"8BIM" {
            return Err("レイヤー情報が壊れています".into());
        }
        let blend: [u8; 4] = r.take(4)?.try_into().unwrap();
        let opacity = r.u8()?;
        let clipping = r.u8()? != 0;
        let flags = r.u8()?;
        r.u8()?;
        let extra_len = r.u32()? as usize;
        let mut e = Reader { d: r.take(extra_len)?, pos: 0 };
        // レイヤーマスク
        let mask_len = e.u32()? as usize;
        let mut mask = None;
        if mask_len > 0 {
            let mut m = Reader { d: e.take(mask_len)?, pos: 0 };
            let mrect = Rect { top: m.i32()?, left: m.i32()?, bottom: m.i32()?, right: m.i32()? };
            let default = m.u8()?;
            let mflags = m.u8()?;
            mask = Some((mrect, default, mflags & 2 != 0));
        }
        let ranges = e.u32()? as usize;
        e.take(ranges)?;
        let mut name = pascal_padded(&mut e, 4)?;
        let mut section = Section::None;
        let mut extras = Vec::new();
        while e.pos + 12 <= e.d.len() {
            let sig = e.take(4)?;
            if sig != b"8BIM" && sig != b"8B64" {
                break;
            }
            let key: [u8; 4] = e.take(4)?.try_into().unwrap();
            // PSB では一部のキーだけ長さが 64bit
            let long = big
                && matches!(
                    &key,
                    b"LMsk"
                        | b"Lr16"
                        | b"Lr32"
                        | b"Layr"
                        | b"Mt16"
                        | b"Mt32"
                        | b"Mtrn"
                        | b"Alph"
                        | b"FMsk"
                        | b"lnk2"
                        | b"FEid"
                        | b"FXid"
                        | b"PxSD"
                );
            let len = e.len(long)?;
            let body = e.take(len.min(e.d.len() - e.pos))?;
            if len % 2 == 1 && e.pos < e.d.len() {
                e.pos += 1;
            }
            match &key {
                b"luni" if body.len() >= 4 => {
                    let n = u32::from_be_bytes(body[..4].try_into().unwrap()) as usize;
                    let units: Vec<u16> = body[4..].chunks_exact(2).take(n).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                    name = String::from_utf16_lossy(&units).trim_end_matches('\0').to_string();
                }
                b"lsct" | b"lsdk" if body.len() >= 4 => {
                    section = match u32::from_be_bytes(body[..4].try_into().unwrap()) {
                        1 | 2 => Section::Folder,
                        3 => Section::Divider,
                        _ => Section::None,
                    };
                }
                _ => extras.push(key),
            }
        }
        records.push(Record {
            rect,
            channels,
            blend,
            opacity,
            clipping,
            hidden: flags & 2 != 0,
            mask,
            name,
            section,
            extras,
            planes: Vec::new(),
        });
    }
    // チャンネルの画像データ（レイヤー順・チャンネル順に並んでいる）
    for rec in &mut records {
        for &(id, len) in &rec.channels {
            let data = r.take(len)?;
            let rect = match (id, rec.mask) {
                (-2, Some((m, _, _))) => m,
                (-3, _) => continue,
                _ => rec.rect,
            };
            if rect.w() == 0 || rect.h() == 0 || len < 2 {
                continue;
            }
            // 色チャンネルの数を超える ID（スポットカラー等）は使わない
            if id >= color_channels as i16 {
                continue;
            }
            rec.planes.push((id, decode_channel(data, rect.w(), rect.h(), depth, big)?));
        }
    }
    Ok(records)
}

/// PSD / PSB を解析する。
pub fn parse_psd(data: &[u8]) -> Result<PsdDocument, String> {
    let mut r = Reader { d: data, pos: 0 };
    if r.take(4)? != b"8BPS" {
        return Err("PSD ファイルではありません".into());
    }
    let big = match r.u16()? {
        1 => false,
        2 => true,
        v => return Err(format!("未対応の PSD バージョンです（{v}）")),
    };
    r.take(6)?;
    let _channels = r.u16()?;
    let (height, width) = (r.u32()?, r.u32()?);
    let depth = r.u16()?;
    let mode = r.u16()?;
    let color_channels = match mode {
        3 => 3,
        1 => 1,
        4 => return Err("CMYK の PSD は未対応です。RGB に変換して保存してください".into()),
        m => return Err(format!("未対応のカラーモードです（{m}）。RGB で保存してください")),
    };
    if depth != 8 && depth != 16 {
        return Err(format!("{depth}bit の PSD は未対応です（8bit または 16bit で保存してください）"));
    }
    let n = r.u32()? as usize;
    r.take(n)?;
    let n = r.u32()? as usize;
    r.take(n)?;

    let section_len = r.len(big)?;
    let mut s = Reader { d: r.take(section_len.min(data.len() - r.pos))?, pos: 0 };
    let mut records = Vec::new();
    if !s.d.is_empty() {
        let info_len = s.len(big)?;
        let mut info = Reader { d: s.take(info_len)?, pos: 0 };
        if info_len > 0 {
            records = read_layer_info(&mut info, big, depth, color_channels)?;
        }
        // 全体のレイヤーマスク情報
        if s.pos + 4 <= s.d.len() {
            let n = s.u32()? as usize;
            s.take(n.min(s.d.len() - s.pos))?;
        }
        // 16bit 以上の文書では、レイヤーがここに入っている
        while s.pos + 12 <= s.d.len() {
            let sig = s.take(4)?;
            if sig != b"8BIM" && sig != b"8B64" {
                break;
            }
            let key: [u8; 4] = s.take(4)?.try_into().unwrap();
            let long = big
                && matches!(
                    &key,
                    b"LMsk"
                        | b"Lr16"
                        | b"Lr32"
                        | b"Layr"
                        | b"Mt16"
                        | b"Mt32"
                        | b"Mtrn"
                        | b"Alph"
                        | b"FMsk"
                        | b"lnk2"
                        | b"FEid"
                        | b"FXid"
                        | b"PxSD"
                );
            let len = s.len(long)?;
            let body = s.take(len.min(s.d.len() - s.pos))?;
            // ここでは長さが 4 の倍数に切り上げられている
            s.pos = (s.pos + (4 - len % 4) % 4).min(s.d.len());
            if matches!(&key, b"Lr16" | b"Lr32") && records.is_empty() {
                records = read_layer_info(&mut Reader { d: body, pos: 0 }, big, depth, color_channels)?;
            }
        }
    }

    let mut warnings = BTreeSet::new();
    let mut layers = Vec::new();
    // グループの見出しは中身より後（上）に現れるので、上から辿って所属を決める
    struct Group {
        name: String,
        opacity: f64,
        visible: bool,
    }
    let mut stack: Vec<Group> = Vec::new();
    for rec in records.iter().rev() {
        match rec.section {
            Section::Folder => {
                if &rec.blend != b"pass" && &rec.blend != b"norm" {
                    warnings.insert(format!("グループ「{}」の描画モードは無視しました（グループは通過として扱います）", rec.name));
                }
                if rec.opacity < 255 {
                    warnings.insert(format!("グループ「{}」の不透明度は各レイヤーに掛けています（重なる部分は元と異なります）", rec.name));
                }
                if rec.mask.is_some() {
                    warnings.insert(format!("グループ「{}」のマスクは無視しました", rec.name));
                }
                stack.push(Group { name: rec.name.clone(), opacity: rec.opacity as f64 / 255.0, visible: !rec.hidden });
                continue;
            }
            Section::Divider => {
                stack.pop();
                continue;
            }
            Section::None => {}
        }
        for key in &rec.extras {
            if let Some(label) = extra_label(key) {
                warnings.insert(format!("レイヤー「{}」: {label}は再現できません", rec.name));
            }
        }
        let (w, h) = (rec.rect.w(), rec.rect.h());
        if w == 0 || h == 0 {
            // 画素を持たないレイヤー（調整レイヤー等）
            continue;
        }
        let plane = |id: i16| rec.planes.iter().find(|(i, _)| *i == id).map(|(_, p)| p);
        let mut rgba = vec![0.0f32; w * h * 4];
        let (r0, g0, b0) = match color_channels {
            1 => (plane(0), plane(0), plane(0)),
            _ => (plane(0), plane(1), plane(2)),
        };
        let alpha = plane(-1);
        for i in 0..w * h {
            let at = |p: Option<&Vec<f32>>, d: f32| p.and_then(|p| p.get(i).copied()).unwrap_or(d);
            rgba[i * 4] = at(r0, 0.0);
            rgba[i * 4 + 1] = at(g0, 0.0);
            rgba[i * 4 + 2] = at(b0, 0.0);
            rgba[i * 4 + 3] = at(alpha, 1.0);
        }
        if let (Some((mrect, default, disabled)), Some(mask)) = (rec.mask, plane(-2)) {
            if !disabled {
                let (mw, mh) = (mrect.w() as i32, mrect.h() as i32);
                let outside = default as f32 / 255.0;
                for y in 0..h as i32 {
                    for x in 0..w as i32 {
                        let (mx, my) = (x + rec.rect.left - mrect.left, y + rec.rect.top - mrect.top);
                        let m = if mx >= 0 && my >= 0 && mx < mw && my < mh { mask[(my * mw + mx) as usize] } else { outside };
                        rgba[(y as usize * w + x as usize) * 4 + 3] *= m;
                    }
                }
            }
        }
        let blend = blend_of(&rec.blend).unwrap_or_else(|| {
            warnings.insert(format!(
                "レイヤー「{}」: 描画モード「{}」は未対応のため通常にしました",
                rec.name,
                String::from_utf8_lossy(&rec.blend).trim()
            ));
            BlendMode::Normal
        });
        layers.push(PsdLayer {
            name: rec.name.clone(),
            left: rec.rect.left,
            top: rec.rect.top,
            width: w,
            height: h,
            rgba,
            opacity: rec.opacity as f64 / 255.0 * stack.iter().map(|g| g.opacity).product::<f64>(),
            visible: !rec.hidden && stack.iter().all(|g| g.visible),
            blend,
            clipping: rec.clipping,
            groups: stack.iter().map(|g| g.name.clone()).collect(),
        });
    }
    layers.reverse();
    if layers.is_empty() {
        return Err("読み込めるレイヤーがありません（統合された画像だけの PSD は、PNG などに書き出して読み込んでください）".into());
    }
    Ok(PsdDocument { width, height, depth, layers, warnings: warnings.into_iter().collect() })
}

fn safe_name(s: &str) -> String {
    let cleaned: String = s.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(40).collect();
    if cleaned.is_empty() {
        "layer".into()
    } else {
        cleaned
    }
}

/// PSD を読み込み、レイヤーごとの PNG を `<PSD と同じ場所>/<名前>_layers/` に書き出して、
/// レイヤー構成を再現したコンポをプロジェクトに追加する。
pub fn import_psd(project: &mut Project, path: &Path, frame_rate: FrameRate, duration: Time) -> Result<(CompId, Vec<String>), MediaError> {
    let err = |message: String| MediaError::File { path: path.display().to_string(), message };
    let data = std::fs::read(path).map_err(|e| err(e.to_string()))?;
    let doc = parse_psd(&data).map_err(err)?;
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "psd".into());
    let dir = path.with_file_name(format!("{stem}_layers"));
    std::fs::create_dir_all(&dir).map_err(|e| err(e.to_string()))?;

    let comp_id = project.fresh_id("comp");
    let mut comp = Composition::new(comp_id.clone(), stem.clone(), doc.width, doc.height, frame_rate, duration);
    let mut ids: Vec<LayerId> = Vec::new();
    for (i, l) in doc.layers.iter().enumerate() {
        let file = dir.join(format!("{:03}_{}.png", i, safe_name(&l.name)));
        let (w, h) = (l.width as u32, l.height as u32);
        let saved = if doc.depth == 16 {
            let px: Vec<u16> = l.rgba.iter().map(|v| (v.clamp(0.0, 1.0) * 65535.0).round() as u16).collect();
            image::ImageBuffer::<image::Rgba<u16>, _>::from_raw(w, h, px).map(|b| image::DynamicImage::ImageRgba16(b).save(&file))
        } else {
            let px: Vec<u8> = l.rgba.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect();
            image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(w, h, px).map(|b| image::DynamicImage::ImageRgba8(b).save(&file))
        };
        match saved {
            Some(Ok(())) => {}
            Some(Err(e)) => return Err(MediaError::File { path: file.display().to_string(), message: e.to_string() }),
            None => return Err(err("レイヤー画像の大きさが不正です".into())),
        }
        let fid = FootageId::new(project.fresh_id("footage"));
        project.footage.push(FootageItem {
            id: fid.clone(),
            name: format!("{stem}/{}", l.name),
            path: file.display().to_string(),
            width: w,
            height: h,
            frame_rate: None,
            duration: None,
            rights: Rights::default(),
            audio: None,
            vfr: false,
        });
        let mut tr = Transform::centered(w as f64, h as f64, doc.width as f64, doc.height as f64);
        tr.position = Property::spatial([l.left as f64 + w as f64 / 2.0, l.top as f64 + h as f64 / 2.0]);
        tr.opacity = Property::constant(l.opacity * 100.0);
        let name = if l.groups.is_empty() { l.name.clone() } else { format!("{} / {}", l.groups.join(" / "), l.name) };
        let lid = format!("{comp_id}-L{i}");
        let mut layer = Layer::new(lid.clone(), name, LayerKind::Footage { footage: fid }, tr, duration);
        layer.blend_mode = l.blend;
        layer.visible = l.visible;
        if l.clipping {
            // クリッピング: 直下の「クリッピングされていない」レイヤーの不透明部分にだけ描く
            let base = (0..i).rev().find(|j| !doc.layers[*j].clipping);
            if let Some(b) = base {
                layer.track_matte = Some(TrackMatte { layer: ids[b].clone(), mode: MatteMode::Alpha });
            }
        }
        ids.push(LayerId::new(lid));
        // コンポのレイヤーは先頭が最前面
        comp.layers.insert(0, layer);
    }
    project.compositions.push(comp);
    Ok((CompId::new(comp_id), doc.warnings))
}
