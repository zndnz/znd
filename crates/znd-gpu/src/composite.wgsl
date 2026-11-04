// 合成の GPU 実装。CPU リファレンス（znd-render/src/raster.rs の rasterize と blend.rs の blend_pixel）と
// 同じ式を、同じ順序で計算する。テクスチャは RGBA32Float（リニアライト・プリマルチプライド）。
//
// 1 回のディスパッチで、レイヤー 1 枚（ta）をキャンバス（ストレージバッファ）にその場で重ねる。
// 処理するのはレイヤーの外接矩形だけ（ステンシルは全面）。ディスパッチの順に重なる（WebGPU が順序を保証する）。

struct Params {
  v: array<vec4<f32>, 12>,
}

@group(0) @binding(0) var ta: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> canvas: array<vec4<f32>>;
@group(0) @binding(2) var<uniform> u: Params;

const LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

fn luma(c: vec3<f32>) -> f32 {
  return dot(c, LUMA);
}

fn srgb_to_linear(c: f32) -> f32 {
  let a = abs(c);
  var l: f32;
  if (a <= 0.04045) {
    l = a / 12.92;
  } else {
    l = pow((a + 0.055) / 1.055, 2.4);
  }
  return select(l, -l, c < 0.0);
}

fn linear_to_srgb(c: f32) -> f32 {
  let a = abs(c);
  var s: f32;
  if (a <= 0.0031308) {
    s = a * 12.92;
  } else {
    s = 1.055 * pow(a, 1.0 / 2.4) - 0.055;
  }
  return select(s, -s, c < 0.0);
}

fn to_display(p: vec4<f32>) -> vec4<f32> {
  let a = p.w;
  if (a <= 0.0) {
    return p;
  }
  return vec4<f32>(linear_to_srgb(p.x / a) * a, linear_to_srgb(p.y / a) * a, linear_to_srgb(p.z / a) * a, a);
}

fn to_linear(p: vec4<f32>) -> vec4<f32> {
  let a = p.w;
  if (a <= 0.0) {
    return p;
  }
  return vec4<f32>(srgb_to_linear(p.x / a) * a, srgb_to_linear(p.y / a) * a, srgb_to_linear(p.z / a) * a, a);
}

// ---------- 描画モード（blend.rs と同じ。番号は BlendMode の宣言順） ----------

fn dodge(b: f32, s: f32) -> f32 {
  if (b <= 0.0) {
    return 0.0;
  } else if (s >= 1.0) {
    return 1.0;
  }
  return min(b / (1.0 - s), 1.0);
}

fn burn(b: f32, s: f32) -> f32 {
  if (b >= 1.0) {
    return 1.0;
  } else if (s <= 0.0) {
    return 0.0;
  }
  return 1.0 - min((1.0 - b) / s, 1.0);
}

fn hard_light(b: f32, s: f32) -> f32 {
  if (s <= 0.5) {
    return b * 2.0 * s;
  }
  let s2 = 2.0 * s - 1.0;
  return b + s2 - b * s2;
}

fn channel(b: f32, s: f32, mode: u32) -> f32 {
  switch mode {
    case 1u: { return min(b, s); }
    case 7u: { return max(b, s); }
    case 9u: { return dodge(b, s); }
    case 3u: { return burn(b, s); }
    case 4u: { return max(b + s - 1.0, 0.0); }
    case 11u: { return hard_light(s, b); }
    case 13u: { return hard_light(b, s); }
    case 12u: {
      if (s <= 0.5) {
        return b - (1.0 - 2.0 * s) * b * (1.0 - b);
      }
      var g: f32;
      if (b <= 0.25) {
        g = ((16.0 * b - 12.0) * b + 4.0) * b;
      } else {
        g = sqrt(b);
      }
      return b + (2.0 * s - 1.0) * (g - b);
    }
    case 14u: { return clamp(b + 2.0 * s - 1.0, 0.0, 1.0); }
    case 15u: {
      if (s <= 0.5) {
        return burn(b, 2.0 * s);
      }
      return dodge(b, 2.0 * s - 1.0);
    }
    case 16u: {
      if (s <= 0.5) {
        return min(b, 2.0 * s);
      }
      return max(b, 2.0 * s - 1.0);
    }
    case 17u: { return select(0.0, 1.0, b + s >= 1.0); }
    case 18u: { return abs(b - s); }
    case 19u: { return b + s - 2.0 * b * s; }
    case 20u: { return max(b - s, 0.0); }
    case 21u: {
      if (s <= 0.0) {
        return select(0.0, 1.0, b > 0.0);
      }
      return min(b / s, 1.0);
    }
    default: { return s; }
  }
}

fn sat(c: vec3<f32>) -> f32 {
  return max(max(c.x, c.y), c.z) - min(min(c.x, c.y), c.z);
}

fn clip_color(c: vec3<f32>) -> vec3<f32> {
  let l = luma(c);
  let n = min(min(c.x, c.y), c.z);
  let x = max(max(c.x, c.y), c.z);
  var out = c;
  if (n < 0.0) {
    out = l + (out - l) * l / (l - n);
  }
  if (x > 1.0) {
    out = l + (out - l) * (1.0 - l) / (x - l);
  }
  return out;
}

fn set_lum(c: vec3<f32>, l: f32) -> vec3<f32> {
  let d = l - luma(c);
  return clip_color(c + vec3<f32>(d));
}

fn set_sat(c: vec3<f32>, s: f32) -> vec3<f32> {
  let n = min(min(c.x, c.y), c.z);
  let x = max(max(c.x, c.y), c.z);
  if (x <= n) {
    return vec3<f32>(0.0);
  }
  let k = s / (x - n);
  return (c - vec3<f32>(n)) * k;
}

fn mix_mode(cb_in: vec3<f32>, cs_in: vec3<f32>, mode: u32) -> vec3<f32> {
  // 暗く・明るく・差・減算・カラー比較は HDR の値をそのまま使う
  let hdr_safe = mode == 1u || mode == 7u || mode == 18u || mode == 20u || mode == 5u || mode == 10u;
  var cb = cb_in;
  var cs = cs_in;
  if (!hdr_safe) {
    cb = clamp(cb, vec3<f32>(0.0), vec3<f32>(1.0));
    cs = clamp(cs, vec3<f32>(0.0), vec3<f32>(1.0));
  }
  switch mode {
    case 5u: { return select(cb, cs, luma(cs) < luma(cb)); }
    case 10u: { return select(cb, cs, luma(cs) > luma(cb)); }
    case 22u: { return set_lum(set_sat(cs, sat(cb)), luma(cb)); }
    case 23u: { return set_lum(set_sat(cb, sat(cs)), luma(cb)); }
    case 24u: { return set_lum(cs, luma(cb)); }
    case 25u: { return set_lum(cb, luma(cs)); }
    default: { return vec3<f32>(channel(cb.x, cs.x, mode), channel(cb.y, cs.y, mode), channel(cb.z, cs.z, mode)); }
  }
}

fn blend_pixel(d: vec4<f32>, s: vec4<f32>, mode: u32) -> vec4<f32> {
  let sa = s.w;
  let da = d.w;
  switch mode {
    case 0u: {
      let k = 1.0 - sa;
      return vec4<f32>(s.xyz + d.xyz * k, sa + da * k);
    }
    case 6u: { return vec4<f32>(s.xyz + d.xyz, sa + da - sa * da); }
    case 2u: {
      let ks = 1.0 - da;
      let kd = 1.0 - sa;
      return vec4<f32>(s.xyz * d.xyz + s.xyz * ks + d.xyz * kd, sa + da - sa * da);
    }
    case 8u: { return s + d - s * d; }
    case 30u: { return vec4<f32>(s.xyz + d.xyz, min(sa + da, 1.0)); }
    case 26u: { return d * sa; }
    case 27u: { return d * clamp(luma(s.xyz), 0.0, 1.0); }
    case 28u: { return d * (1.0 - sa); }
    case 29u: { return d * (1.0 - clamp(luma(s.xyz), 0.0, 1.0)); }
    default: {
      if (sa <= 0.0) {
        return d;
      }
      if (da <= 0.0) {
        return s;
      }
      let cs = s.xyz / sa;
      let cb = d.xyz / da;
      let b = mix_mode(cb, cs, mode);
      let both = sa * da;
      return vec4<f32>((1.0 - da) * s.xyz + (1.0 - sa) * d.xyz + both * b, sa + da - both);
    }
  }
}

// ---------- ソースの取り出し（raster.rs と同じ） ----------

fn overlap(c: f32, f: f32, size: f32) -> f32 {
  let lo = max(c - 0.5 * f, 0.0);
  let hi = min(c + 0.5 * f, size);
  return clamp((hi - lo) / f, 0.0, 1.0);
}

// 画素中心が +0.5。画像の外は端の画素を延長する
fn bilinear(sx: f32, sy: f32) -> vec4<f32> {
  let d = vec2<i32>(textureDimensions(ta));
  let uu = clamp(sx - 0.5, 0.0, f32(d.x - 1));
  let vv = clamp(sy - 0.5, 0.0, f32(d.y - 1));
  let x0 = i32(uu);
  let y0 = i32(vv);
  let x1 = min(x0 + 1, d.x - 1);
  let y1 = min(y0 + 1, d.y - 1);
  let fu = uu - f32(x0);
  let fv = vv - f32(y0);
  let p00 = textureLoad(ta, vec2<i32>(x0, y0), 0);
  let p10 = textureLoad(ta, vec2<i32>(x1, y0), 0);
  let p01 = textureLoad(ta, vec2<i32>(x0, y1), 0);
  let p11 = textureLoad(ta, vec2<i32>(x1, y1), 0);
  let top = p00 + (p10 - p00) * fu;
  let bot = p01 + (p11 - p01) * fu;
  return top + (bot - top) * fv;
}

fn inv_apply(x: f32, y: f32) -> vec2<f32> {
  let m = u.v[0];
  let t = u.v[1];
  return vec2<f32>(m.x * x + m.z * y + t.x, m.y * x + m.w * y + t.y);
}

fn source(px: vec2<i32>) -> vec4<f32> {
  let kind = u32(u.v[4].x);
  let bbox = vec4<i32>(u.v[5]);
  if (kind == 2u) {
    // 出力空間のタイル
    if (px.x < bbox.x || px.y < bbox.y || px.x >= bbox.z || px.y >= bbox.w) {
      return vec4<f32>(0.0);
    }
    return textureLoad(ta, px - bbox.xy, 0);
  }
  let size = u.v[1].zw;
  let f = u.v[2].xy;
  let n = vec2<i32>(u.v[2].zw);
  let opacity = u.v[4].y;
  let pxc = f32(px.x) + 0.5;
  let py = f32(px.y) + 0.5;
  let s = inv_apply(pxc, py);
  let cov = overlap(s.x, f.x, size.x) * overlap(s.y, f.y, size.y) * opacity;
  if (cov <= 0.0) {
    return vec4<f32>(0.0);
  }
  var c: vec4<f32>;
  if (kind == 0u) {
    c = u.v[3];
  } else if (n.x == 1 && n.y == 1) {
    c = bilinear(s.x, s.y);
  } else {
    var acc = vec4<f32>(0.0);
    for (var j = 0; j < n.y; j++) {
      let oy = (f32(j) + 0.5) / f32(n.y) - 0.5;
      for (var i = 0; i < n.x; i++) {
        let ox = (f32(i) + 0.5) / f32(n.x) - 0.5;
        let a = inv_apply(pxc + ox, py + oy);
        acc += bilinear(a.x, a.y);
      }
    }
    c = acc / f32(n.x * n.y);
  }
  return c * cov;
}

// u.v[6] = 処理する範囲 (x0, y0, x1, y1)、u.v[7].x = キャンバスの幅
@compute @workgroup_size(8, 8)
fn cs_place(@builtin(global_invocation_id) gid: vec3<u32>) {
  let region = vec4<i32>(u.v[6]);
  let px = vec2<i32>(gid.xy) + region.xy;
  if (px.x >= region.z || px.y >= region.w) {
    return;
  }
  let index = u32(px.y) * u32(u.v[7].x) + u32(px.x);
  let d = canvas[index];
  let mode = u32(u.v[4].z);
  let flags = u32(u.v[4].w);
  let linear = (flags & 1u) != 0u;
  let clamped = (flags & 2u) != 0u;
  let stencil = mode == 26u || mode == 27u;
  let bbox = vec4<i32>(u.v[5]);
  let inside = px.x >= bbox.x && px.y >= bbox.y && px.x < bbox.z && px.y < bbox.w;
  if (stencil && !inside) {
    // ステンシルはレイヤーの外の絵を消す
    canvas[index] = vec4<f32>(0.0);
    return;
  }
  let s = source(px);
  var out: vec4<f32>;
  if (!stencil && all(s == vec4<f32>(0.0))) {
    out = d;
  } else if (linear) {
    out = blend_pixel(d, s, mode);
  } else {
    out = to_linear(blend_pixel(to_display(d), to_display(s), mode));
  }
  if (clamped && inside) {
    out = clamp(out, vec4<f32>(0.0), vec4<f32>(1.0));
  }
  canvas[index] = out;
}
