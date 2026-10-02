// エフェクトの GPU 実装。CPU リファレンス（znd-render/src/effects）と同じ式を、同じ順序で計算する。
// テクスチャは RGBA32Float（リニアライト・プリマルチプライド）。補間は自前で行う（textureLoad のみ使用）。

struct Params {
  v: array<vec4<f32>, 12>,
}

@group(0) @binding(0) var ta: texture_2d<f32>;
@group(0) @binding(1) var tb: texture_2d<f32>;
@group(0) @binding(2) var<uniform> u: Params;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
  let x = f32(i32(i & 1u) * 4 - 1);
  let y = f32(i32(i >> 1u) * 4 - 1);
  return vec4<f32>(x, y, 0.0, 1.0);
}

const TAU: f32 = 6.283185307179586;

fn size_a() -> vec2<i32> {
  return vec2<i32>(textureDimensions(ta));
}

// 画像の外は透明
fn la(x: i32, y: i32) -> vec4<f32> {
  let d = size_a();
  if (x < 0 || y < 0 || x >= d.x || y >= d.y) {
    return vec4<f32>(0.0);
  }
  return textureLoad(ta, vec2<i32>(x, y), 0);
}

// 端の画素を延長する
fn la_clamp(x: i32, y: i32) -> vec4<f32> {
  let d = size_a();
  return textureLoad(ta, vec2<i32>(clamp(x, 0, d.x - 1), clamp(y, 0, d.y - 1)), 0);
}

fn lb(x: i32, y: i32) -> vec4<f32> {
  return textureLoad(tb, vec2<i32>(x, y), 0);
}

// バイリニア（画素中心が +0.5）。画像の外は透明
fn sample_a(x: f32, y: f32) -> vec4<f32> {
  let uu = x - 0.5;
  let vv = y - 0.5;
  let x0 = floor(uu);
  let y0 = floor(vv);
  let fu = uu - x0;
  let fv = vv - y0;
  let ix = i32(clamp(x0, -1.0e6, 1.0e6));
  let iy = i32(clamp(y0, -1.0e6, 1.0e6));
  let a = la(ix, iy);
  let b = la(ix + 1, iy);
  let c = la(ix, iy + 1);
  let d = la(ix + 1, iy + 1);
  let top = a + (b - a) * fu;
  let bot = c + (d - c) * fu;
  return top + (bot - top) * fv;
}

fn sample_clamp_a(x: f32, y: f32) -> vec4<f32> {
  let d = vec2<f32>(size_a());
  return sample_a(clamp(x, 0.5, d.x - 0.5), clamp(y, 0.5, d.y - 0.5));
}

fn l2s(c: f32) -> f32 {
  let a = abs(c);
  var s: f32;
  if (a <= 0.0031308) {
    s = a * 12.92;
  } else {
    s = 1.055 * pow(a, 1.0 / 2.4) - 0.055;
  }
  if (c < 0.0) {
    return -s;
  }
  return s;
}

fn s2l(c: f32) -> f32 {
  let a = abs(c);
  var l: f32;
  if (a <= 0.04045) {
    l = a / 12.92;
  } else {
    l = pow((a + 0.055) / 1.055, 2.4);
  }
  if (c < 0.0) {
    return -l;
  }
  return l;
}

fn to_display(px: vec4<f32>) -> vec3<f32> {
  if (px.a <= 0.0) {
    return vec3<f32>(0.0);
  }
  return vec3<f32>(l2s(px.r / px.a), l2s(px.g / px.a), l2s(px.b / px.a));
}

fn from_display(c: vec3<f32>, a: f32) -> vec4<f32> {
  return vec4<f32>(s2l(c.r) * a, s2l(c.g) * a, s2l(c.b) * a, a);
}

fn luma(c: vec3<f32>) -> f32 {
  return c.r * 0.2126 + c.g * 0.7152 + c.b * 0.0722;
}

fn display_luma(px: vec4<f32>) -> f32 {
  return luma(to_display(px));
}

fn sstep(e0: f32, e1: f32, x: f32) -> f32 {
  if (e1 <= e0) {
    if (x >= e0) {
      return 1.0;
    }
    return 0.0;
  }
  let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
  return t * t * (3.0 - 2.0 * t);
}

fn rem(a: f32, b: f32) -> f32 {
  return a - b * floor(a / b);
}

fn rgb_to_hsl(c: vec3<f32>) -> vec3<f32> {
  let mx = max(c.r, max(c.g, c.b));
  let mn = min(c.r, min(c.g, c.b));
  let l = (mx + mn) / 2.0;
  let d = mx - mn;
  if (d <= 1.0e-9) {
    return vec3<f32>(0.0, 0.0, l);
  }
  let s = d / max(1.0 - abs(2.0 * l - 1.0), 1.0e-9);
  var h: f32;
  if (mx == c.r) {
    h = rem((c.g - c.b) / d, 6.0);
  } else if (mx == c.g) {
    h = (c.b - c.r) / d + 2.0;
  } else {
    h = (c.r - c.g) / d + 4.0;
  }
  return vec3<f32>(h * 60.0, min(s, 1.0), l);
}

fn hsl_to_rgb(c: vec3<f32>) -> vec3<f32> {
  let h = rem(c.x, 360.0) / 60.0;
  let s = c.y;
  let l = c.z;
  let chroma = (1.0 - abs(2.0 * l - 1.0)) * s;
  let x = chroma * (1.0 - abs(rem(h, 2.0) - 1.0));
  var rgb: vec3<f32>;
  let k = u32(h);
  if (k == 0u) {
    rgb = vec3<f32>(chroma, x, 0.0);
  } else if (k == 1u) {
    rgb = vec3<f32>(x, chroma, 0.0);
  } else if (k == 2u) {
    rgb = vec3<f32>(0.0, chroma, x);
  } else if (k == 3u) {
    rgb = vec3<f32>(0.0, x, chroma);
  } else if (k == 4u) {
    rgb = vec3<f32>(x, 0.0, chroma);
  } else {
    rgb = vec3<f32>(chroma, 0.0, x);
  }
  let m = l - chroma / 2.0;
  return rgb + vec3<f32>(m);
}

// ---------- ぼかし ----------

// 分離カーネル。v0 = (dx, dy, 半径, 端の扱い 0=透明 1=延長)、重みは v1 以降に 4 個ずつ
@fragment
fn fs_conv(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
  let p = vec2<i32>(pos.xy);
  let dir = vec2<i32>(i32(u.v[0].x), i32(u.v[0].y));
  let r = i32(u.v[0].z);
  let edge = u.v[0].w;
  var acc = vec4<f32>(0.0);
  for (var k: i32 = 0; k <= 2 * r; k++) {
    let q = p + dir * (k - r);
    let wgt = u.v[1 + k / 4][k % 4];
    if (edge > 0.5) {
      acc += wgt * la_clamp(q.x, q.y);
    } else {
      acc += wgt * la(q.x, q.y);
    }
  }
  return acc;
}

// 幅 2r+1 のボックスブラー。v0 = (dx, dy, 半径, 端の扱い)
@fragment
fn fs_box(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
  let p = vec2<i32>(pos.xy);
  let dir = vec2<i32>(i32(u.v[0].x), i32(u.v[0].y));
  let r = i32(u.v[0].z);
  let edge = u.v[0].w;
  var acc = vec4<f32>(0.0);
  for (var k: i32 = -r; k <= r; k++) {
    let q = p + dir * k;
    if (edge > 0.5) {
      acc += la_clamp(q.x, q.y);
    } else {
      acc += la(q.x, q.y);
    }
  }
  return acc / f32(2 * r + 1);
}

// 線分・円弧・放射に沿った平均。v0 = (モード, 標本数)
@fragment
fn fs_along(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
  let mode = i32(u.v[0].x);
  let n = i32(u.v[0].y);
  let px = pos.x;
  let py = pos.y;
  var acc = vec4<f32>(0.0);
  if (mode == 3) {
    // 光線: 光源 (cx, cy) へ向かって引き伸ばす。v1 = (cx, cy, 長さ)
    let cx = u.v[1].x;
    let cy = u.v[1].y;
    let len = u.v[1].z;
    for (var i: i32 = 0; i < 48; i++) {
      let t = f32(i) / 47.0;
      let kk = 1.0 - len * t;
      let s = sample_a(cx + (px - cx) * kk, cy + (py - cy) * kk);
      acc += s * (1.0 - t * 0.7);
    }
    return acc / 48.0 * 1.5;
  }
  for (var i: i32 = 0; i < n; i++) {
    var t = 0.5;
    if (n > 1) {
      t = f32(i) / f32(n - 1);
    }
    var sx: f32;
    var sy: f32;
    if (mode == 0) {
      // 方向: v1 = (dx, dy)
      sx = px + u.v[1].x * (t - 0.5);
      sy = py + u.v[1].y * (t - 0.5);
    } else if (mode == 1) {
      // 回転: v1 = (cx, cy, 角度)
      let ang = u.v[1].z * (t - 0.5);
      let sn = sin(ang);
      let cs = cos(ang);
      let dx = px - u.v[1].x;
      let dy = py - u.v[1].y;
      sx = u.v[1].x + dx * cs - dy * sn;
      sy = u.v[1].y + dx * sn + dy * cs;
    } else {
      // ズーム: v1 = (cx, cy, 量)
      let k = 1.0 - u.v[1].z * t;
      sx = u.v[1].x + (px - u.v[1].x) * k;
      sy = u.v[1].y + (py - u.v[1].y) * k;
    }
    acc += sample_a(sx, sy);
  }
  return acc / f32(max(n, 1));
}

// ---------- 画素ごとの色の処理 ----------

fn blend_original(original: vec4<f32>, result: vec4<f32>, t: f32) -> vec4<f32> {
  if (t > 0.0) {
    return result + (original - result) * t;
  }
  return result;
}

@fragment
fn fs_color(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
  let p = vec2<i32>(pos.xy);
  let px = textureLoad(ta, p, 0);
  let mode = i32(u.v[0].x);
  let t = u.v[0].y;

  if (mode == 5) {
    // 露出（リニア値に掛ける）: v1 = (ゲイン, オフセット)
    let gain = u.v[1].x;
    let off = u.v[1].y;
    return vec4<f32>(px.rgb * gain + vec3<f32>(off * px.a), px.a);
  }
  if (mode == 8) {
    // 明るい部分だけを残す: v1.x = しきい値
    let k = sstep(u.v[1].x - 0.1, u.v[1].x + 0.1, display_luma(px));
    return px * k;
  }
  if (mode == 10) {
    // ルミナンスキー: v1 = (しきい値, 柔らかさ, 明るい側を抜くか)
    let l = display_luma(px);
    var k = sstep(u.v[1].x - u.v[1].y, u.v[1].x + u.v[1].y, l);
    if (u.v[1].z > 0.5) {
      k = 1.0 - k;
    }
    return px * k;
  }
  if (mode == 12) {
    // 走査線: v1 = (間隔, 濃さ, スクロール量)
    let spacing = u.v[1].x;
    let phase = rem((f32(p.y) + 0.5 + u.v[1].z) / spacing, 1.0);
    let edge = min(0.5 / spacing, 0.25);
    let line = sstep(0.5 - edge, 0.5 + edge, phase);
    let k = 1.0 - u.v[1].y * line;
    return vec4<f32>(px.rgb * k, px.a);
  }
  if (mode == 14) {
    // アルファのしきい値化（マットチョークの途中段）
    return vec4<f32>(0.0, 0.0, 0.0, sstep(u.v[1].x - 0.1, u.v[1].x + 0.1, px.a));
  }
  if (px.a <= 0.0) {
    return px;
  }
  if (mode == 15) {
    // マットチョークの仕上げ: tb のアルファに合わせて全体を掛け直す
    let alpha = lb(p.x, p.y).a;
    var limit = 1.0;
    if (u.v[1].x > 0.5) {
      limit = 1.0 / px.a;
    }
    return px * min(alpha / px.a, limit);
  }
  if (mode == 11) {
    // クロマキー: v1 = (キーの Cb, Cr, 許容量, 柔らかさ)、v2 = (スピル除去, 優勢チャンネル, 他 2 つ)
    var d = to_display(px);
    let y = 0.2126 * d.r + 0.7152 * d.g + 0.0722 * d.b;
    let cb = (d.b - y) / 1.8556;
    let cr = (d.r - y) / 1.5748;
    let dist = length(vec2<f32>(cb - u.v[1].x, cr - u.v[1].y));
    let alpha = sstep(u.v[1].z, u.v[1].z + u.v[1].w, dist);
    let dom = i32(u.v[2].y);
    let limit = (d[i32(u.v[2].z)] + d[i32(u.v[2].w)]) / 2.0;
    if (d[dom] > limit) {
      d[dom] += (limit - d[dom]) * u.v[2].x;
    }
    return from_display(d, px.a * alpha);
  }
  if (mode == 13) {
    // シャドウ・ハイライト: tb = ぼかした画像、v1 = (シャドウ量, ハイライト量)
    let l = clamp(display_luma(lb(p.x, p.y)), 0.0, 1.0);
    let d = to_display(px);
    let lift = u.v[1].x * (1.0 - l) * (1.0 - l);
    let cut = u.v[1].y * l * l;
    return from_display(d + (vec3<f32>(1.0) - d) * lift * 0.6 - d * cut * 0.6, px.a);
  }

  let c = to_display(px);
  var o = c;
  if (mode == 0) {
    // レベル: v1 = (入力黒, 範囲, 1/ガンマ)、v2 = (出力黒, 出力白)
    for (var k: i32 = 0; k < 3; k++) {
      let n = max((c[k] - u.v[1].x) / u.v[1].y, 0.0);
      var g = n;
      if (n <= 1.0) {
        g = pow(n, u.v[1].z);
      }
      o[k] = u.v[2].x + g * (u.v[2].y - u.v[2].x);
    }
  } else if (mode == 1) {
    // 色相・彩度: v1 = (色相, 彩度, 明度)
    var hsl = rgb_to_hsl(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)));
    hsl.x += u.v[1].x;
    hsl.y = clamp(hsl.y * (1.0 + u.v[1].y), 0.0, 1.0);
    if (u.v[1].z >= 0.0) {
      hsl.z = hsl.z + (1.0 - hsl.z) * u.v[1].z;
    } else {
      hsl.z = hsl.z * (1.0 + u.v[1].z);
    }
    o = hsl_to_rgb(hsl);
  } else if (mode == 2) {
    // カラーバランス: v1 = シャドウ、v2 = 中間調、v3 = ハイライト
    let l = clamp(luma(c), 0.0, 1.0);
    let ws = max(1.0 - l * 2.0, 0.0);
    let wh = max(l * 2.0 - 1.0, 0.0);
    let wm = 1.0 - ws - wh;
    o = c + (u.v[1].rgb * ws + u.v[2].rgb * wm + u.v[3].rgb * wh) * 0.5;
  } else if (mode == 3) {
    // トライトーン: v1 = ハイライト、v2 = 中間調、v3 = シャドウ
    let l = clamp(luma(c), 0.0, 1.0);
    if (l < 0.5) {
      o = u.v[3].rgb + (u.v[2].rgb - u.v[3].rgb) * (l * 2.0);
    } else {
      o = u.v[2].rgb + (u.v[1].rgb - u.v[2].rgb) * (l * 2.0 - 1.0);
    }
  } else if (mode == 4) {
    // グラデーションマップ: v1 = 色 A、v2 = 色 B
    o = u.v[1].rgb + (u.v[2].rgb - u.v[1].rgb) * clamp(luma(c), 0.0, 1.0);
  } else if (mode == 6) {
    // 明るさ・コントラスト: v1 = (明るさ, 傾き)
    o = (c + vec3<f32>(u.v[1].x - 0.5)) * u.v[1].y + vec3<f32>(0.5);
  } else if (mode == 7) {
    o = vec3<f32>(1.0) - c;
  } else if (mode == 9) {
    // ポスタリゼーション: v1.x = 階調数 − 1
    o = floor(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)) * u.v[1].x + vec3<f32>(0.5)) / u.v[1].x;
  }
  return blend_original(px, from_display(o, px.a), t);
}

// ---------- 光 ----------

// ta = 元の画像、tb = 光。v0 = (強さ, 色 RGB)
@fragment
fn fs_add_light(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
  let p = vec2<i32>(pos.xy);
  let b = textureLoad(ta, p, 0);
  let g = lb(p.x, p.y);
  let k = u.v[0].x;
  let rgb = b.rgb + g.rgb * u.v[0].yzw * k;
  let ga = clamp(g.a * k, 0.0, 1.0);
  return vec4<f32>(rgb, b.a + ga - b.a * ga);
}

// ta + tb × v0.x
@fragment
fn fs_add_scaled(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
  let p = vec2<i32>(pos.xy);
  return textureLoad(ta, p, 0) + lb(p.x, p.y) * u.v[0].x;
}

// tb × v0.x（積み上げの最初の 1 枚）
@fragment
fn fs_scaled(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
  let p = vec2<i32>(pos.xy);
  return lb(p.x, p.y) * u.v[0].x;
}

// 影の元: アルファをずらして色を付ける。v0 = (dx, dy, 不透明度)、v1 = 色（プリマルチプライド）
@fragment
fn fs_shadow(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
  let al = sample_a(pos.x - u.v[0].x, pos.y - u.v[0].y).a * u.v[0].z;
  return u.v[1] * al;
}

// ta（元の画像）を tb の上に重ねる
@fragment
fn fs_over(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
  let p = vec2<i32>(pos.xy);
  let o = textureLoad(ta, p, 0);
  return o + lb(p.x, p.y) * (1.0 - o.a);
}

// レンズフレア（光だけを生成する）。v0 = (cx, cy, 明るさ)、v1 = 色（プリマルチプライド）
@fragment
fn fs_flare(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
  let dim = vec2<f32>(size_a());
  let cx = u.v[0].x;
  let cy = u.v[0].y;
  let mx = dim.x / 2.0;
  let my = dim.y / 2.0;
  let size = max(dim.x, dim.y);
  let d = length(vec2<f32>(pos.x - cx, pos.y - cy)) / size;
  var v = exp(-d * d * 900.0) + 0.35 * exp(-d * 14.0);
  v += 0.12 * exp(-pow((d - 0.18) * 60.0, 2.0));
  v += 0.3 * exp(-pow((pos.y - cy) / (size * 0.004), 2.0)) * exp(-pow((pos.x - cx) / (size * 0.35), 2.0));
  var ghosts = array<vec3<f32>, 5>(
    vec3<f32>(0.5, 0.03, 0.25),
    vec3<f32>(0.9, 0.06, 0.12),
    vec3<f32>(1.3, 0.02, 0.3),
    vec3<f32>(1.6, 0.1, 0.08),
    vec3<f32>(2.0, 0.05, 0.15)
  );
  for (var i: i32 = 0; i < 5; i++) {
    let g = ghosts[i];
    let gx = cx + (mx - cx) * g.x;
    let gy = cy + (my - cy) * g.x;
    let gd = length(vec2<f32>(pos.x - gx, pos.y - gy)) / (size * g.y);
    v += g.z * (1.0 - sstep(0.7, 1.0, gd));
  }
  v = v * u.v[0].z;
  return vec4<f32>(u.v[1].rgb * v, clamp(v, 0.0, 1.0));
}

// ---------- 歪み・スタイライズ ----------

fn wave(kind: i32, x: f32) -> f32 {
  let f = rem(x, 1.0);
  if (kind == 1) {
    if (f < 0.5) {
      return 1.0;
    }
    return -1.0;
  }
  if (kind == 2) {
    return 1.0 - 4.0 * abs(f - 0.5);
  }
  if (kind == 3) {
    return 2.0 * f - 1.0;
  }
  return sin(f * TAU);
}

fn irem2(v: f32) -> i32 {
  // floor した値の偶奇（負の値でも 0 か 1）
  return i32(rem(v, 2.0));
}

@fragment
fn fs_warp(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
  let mode = i32(u.v[0].x);
  let dim = vec2<f32>(size_a());
  let x = pos.x;
  let y = pos.y;
  if (mode == 0) {
    // RGB 分離: v1 = (dx, dy, 放射状か, 量)
    var dx = u.v[1].x;
    var dy = u.v[1].y;
    if (u.v[1].z > 0.5) {
      let cx = dim.x / 2.0;
      let cy = dim.y / 2.0;
      let reach = max(length(vec2<f32>(cx, cy)), 1.0);
      dx = (x - cx) / reach * u.v[1].w;
      dy = (y - cy) / reach * u.v[1].w;
    }
    let r = sample_a(x + dx, y + dy);
    let g = sample_a(x, y);
    let b = sample_a(x - dx, y - dy);
    return vec4<f32>(r.r, g.g, b.b, (r.a + g.a + b.a) / 3.0);
  }
  if (mode == 1) {
    // 波形ワープ: v1 = (高さ, 幅, dx, dy)、v2 = (位相, 波の種類)
    let dx = u.v[1].z;
    let dy = u.v[1].w;
    let along = (x * -dy + y * dx) / u.v[1].y;
    let d = wave(i32(u.v[2].y), along - u.v[2].x) * u.v[1].x;
    return sample_a(x - dx * d, y - dy * d);
  }
  if (mode == 2) {
    // 極座標: v1 = (補間, 直交→極か)
    let cx = dim.x / 2.0;
    let cy = dim.y / 2.0;
    let max_r = min(cx, cy);
    var sx: f32;
    var sy: f32;
    if (u.v[1].y > 0.5) {
      let dx = x - cx;
      let dy = y - cy;
      // 中心ちょうどでは atan2(0, 0) が未定義になるので、CPU と同じ値（π）を使う
      var ang = 0.5;
      if (dx != 0.0 || dy != 0.0) {
        // −0 を渡すと実装によって符号の扱いが変わるので、+0 にそろえる（結果は数学的に同じ）
        var ny = -dy;
        if (ny == 0.0) {
          ny = 0.0;
        }
        ang = rem(atan2(dx, ny), TAU) / TAU;
      }
      sx = ang * dim.x;
      sy = length(vec2<f32>(dx, dy)) / max_r * dim.y;
    } else {
      let ang = x / dim.x * TAU;
      let r = y / dim.y * max_r;
      sx = cx + r * sin(ang);
      sy = cy - r * cos(ang);
    }
    return sample_a(x + (sx - x) * u.v[1].x, y + (sy - y) * u.v[1].x);
  }
  if (mode == 3) {
    // ミラー: v1 = (cx, cy, nx, ny)
    let d = (x - u.v[1].x) * u.v[1].z + (y - u.v[1].y) * u.v[1].w;
    if (d > 0.0) {
      return sample_a(x - 2.0 * d * u.v[1].z, y - 2.0 * d * u.v[1].w);
    }
    return sample_a(x, y);
  }
  // モーションタイル: v1 = (cx, cy, タイル幅, タイル高さ)、v2 = (端をミラーするか, 位相)
  let uu = (x - u.v[1].x) / u.v[1].z + 0.5;
  let col = floor(uu);
  var vv = (y - u.v[1].y) / u.v[1].w + 0.5;
  if (irem2(col) == 1) {
    vv += u.v[2].y;
  }
  let row = floor(vv);
  var fu = uu - col;
  var fv = vv - row;
  if (u.v[2].x > 0.5) {
    if (irem2(col) == 1) {
      fu = 1.0 - fu;
    }
    if (irem2(row) == 1) {
      fv = 1.0 - fv;
    }
  }
  return sample_clamp_a(fu * dim.x, fv * dim.y);
}

// 輪郭検出（ソーベル）。v0 = (反転するか, 元の画像と混ぜる割合)
@fragment
fn fs_edges(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
  let p = vec2<i32>(pos.xy);
  let l00 = display_luma(la_clamp(p.x - 1, p.y - 1));
  let l10 = display_luma(la_clamp(p.x, p.y - 1));
  let l20 = display_luma(la_clamp(p.x + 1, p.y - 1));
  let l01 = display_luma(la_clamp(p.x - 1, p.y));
  let l21 = display_luma(la_clamp(p.x + 1, p.y));
  let l02 = display_luma(la_clamp(p.x - 1, p.y + 1));
  let l12 = display_luma(la_clamp(p.x, p.y + 1));
  let l22 = display_luma(la_clamp(p.x + 1, p.y + 1));
  let gx = l20 + 2.0 * l21 + l22 - l00 - 2.0 * l01 - l02;
  let gy = l02 + 2.0 * l12 + l22 - l00 - 2.0 * l10 - l20;
  let e = min(length(vec2<f32>(gx, gy)), 1.0);
  var v = 1.0 - e;
  if (u.v[0].x > 0.5) {
    v = e;
  }
  let px = textureLoad(ta, p, 0);
  return blend_original(px, from_display(vec3<f32>(v), px.a), u.v[0].y);
}
