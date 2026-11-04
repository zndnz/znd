// エクスプレッションの実行環境。After Effects のエクスプレッションと同じ名前・同じ使い方ができるよう、
// ここで JavaScript 側の API を組み立てる。Rust 側は `__` で始まる少数のネイティブ関数だけを提供する。
'use strict';

var __envStack = [];
function __env() { return __envStack[__envStack.length - 1]; }
function __arr(v) { return v.length === 1 ? v[0] : v; }
function __unwrap(x) {
  if (x instanceof Number) return x.valueOf();
  return x;
}
function __vec(x) { x = __unwrap(x); return Array.isArray(x) ? x : [x]; }

// ---- ベクトル演算 ----
function add(a, b) {
  a = __unwrap(a); b = __unwrap(b);
  if (!Array.isArray(a) && !Array.isArray(b)) return a + b;
  a = __vec(a); b = __vec(b);
  var n = Math.max(a.length, b.length), o = [];
  for (var i = 0; i < n; i++) o.push((a[i] || 0) + (b[i] || 0));
  return o;
}
function sub(a, b) {
  a = __unwrap(a); b = __unwrap(b);
  if (!Array.isArray(a) && !Array.isArray(b)) return a - b;
  a = __vec(a); b = __vec(b);
  var n = Math.max(a.length, b.length), o = [];
  for (var i = 0; i < n; i++) o.push((a[i] || 0) - (b[i] || 0));
  return o;
}
function mul(a, b) {
  a = __unwrap(a); b = __unwrap(b);
  if (Array.isArray(a) && Array.isArray(b)) return a.map(function (v, i) { return v * (b[i] === undefined ? 1 : b[i]); });
  if (Array.isArray(a)) return a.map(function (v) { return v * b; });
  if (Array.isArray(b)) return b.map(function (v) { return v * a; });
  return a * b;
}
function div(a, b) {
  a = __unwrap(a); b = __unwrap(b);
  if (Array.isArray(a) && Array.isArray(b)) return a.map(function (v, i) { return v / (b[i] === undefined ? 1 : b[i]); });
  if (Array.isArray(a)) return a.map(function (v) { return v / b; });
  return a / b;
}
function length(a, b) {
  if (b !== undefined) a = sub(a, b);
  a = __vec(a);
  var s = 0;
  for (var i = 0; i < a.length; i++) s += a[i] * a[i];
  return Math.sqrt(s);
}
function normalize(a) { var l = length(a); return l === 0 ? __vec(a).slice() : div(a, l); }
function dot(a, b) { a = __vec(a); b = __vec(b); var s = 0; for (var i = 0; i < a.length; i++) s += a[i] * (b[i] || 0); return s; }
function cross(a, b) {
  a = __vec(a); b = __vec(b);
  var ax = a[0] || 0, ay = a[1] || 0, az = a[2] || 0, bx = b[0] || 0, by = b[1] || 0, bz = b[2] || 0;
  return [ay * bz - az * by, az * bx - ax * bz, ax * by - ay * bx];
}
function clamp(v, lo, hi) {
  v = __unwrap(v); lo = __unwrap(lo); hi = __unwrap(hi);
  if (Array.isArray(v)) return v.map(function (x, i) { return clamp(x, Array.isArray(lo) ? lo[i] : lo, Array.isArray(hi) ? hi[i] : hi); });
  if (lo > hi) { var t = lo; lo = hi; hi = t; }
  return Math.min(Math.max(v, lo), hi);
}
function degreesToRadians(d) { return d * Math.PI / 180; }
function radiansToDegrees(r) { return r * 180 / Math.PI; }

// ---- 補間 ----
function __interp(t, tMin, tMax, v1, v2, shape) {
  if (v1 === undefined) { v1 = tMin; v2 = tMax; tMin = 0; tMax = 1; }
  t = __unwrap(t); tMin = __unwrap(tMin); tMax = __unwrap(tMax);
  var u = tMax === tMin ? (t < tMin ? 0 : 1) : (t - tMin) / (tMax - tMin);
  u = Math.min(Math.max(u, 0), 1);
  u = shape(u);
  return add(v1, mul(sub(v2, v1), u));
}
function linear(t, a, b, v1, v2) { return __interp(t, a, b, v1, v2, function (u) { return u; }); }
function ease(t, a, b, v1, v2) { return __interp(t, a, b, v1, v2, function (u) { return u * u * (3 - 2 * u); }); }
function easeIn(t, a, b, v1, v2) { return __interp(t, a, b, v1, v2, function (u) { return u * u; }); }
function easeOut(t, a, b, v1, v2) { return __interp(t, a, b, v1, v2, function (u) { return 1 - (1 - u) * (1 - u); }); }

// ---- 乱数・ノイズ（決定的） ----
function __hash(n) {
  n = Math.imul(n ^ (n >>> 16), 0x7feb352d);
  n = Math.imul(n ^ (n >>> 15), 0x846ca68b);
  return (n ^ (n >>> 16)) >>> 0;
}
function __strHash(s) { var h = 0x811c9dc5; for (var i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 0x01000193); return h >>> 0; }
function __rnd(a, b) { return __hash(Math.imul(a | 0, 0x9e3779b1) ^ __hash(b | 0)) / 4294967295; }
function __noise1(seed, x) {
  var i = Math.floor(x), f = x - i, s = f * f * f * (f * (f * 6 - 15) + 10);
  var a = __rnd(seed, i) * 2 - 1, b = __rnd(seed, i + 1) * 2 - 1;
  return a + (b - a) * s;
}
function noise(v) {
  v = __vec(v);
  return __noise1(77, v[0] || 0) * 0.5 + __noise1(191, (v[1] || 0) + 13.7) * 0.3 + __noise1(313, (v[2] || 0) - 5.1) * 0.2;
}
function seedRandom(seed, timeless) { var e = __env(); e.rs = { seed: seed | 0, timeless: !!timeless, n: 0 }; }
function __rand01() {
  var e = __env(), rs = e.rs;
  // 既定では「レイヤー・プロパティ・フレーム」ごとに違う列になる。timeless ならフレームに依らない
  var frame = rs.timeless ? 0 : Math.round(e.global.time * 1000);
  return __rnd(Math.imul(e.seed + rs.seed * 7919, 31) + frame, rs.n++);
}
function random(a, b) {
  if (a === undefined) return __rand01();
  a = __unwrap(a); b = __unwrap(b);
  if (b === undefined) { b = a; a = Array.isArray(b) ? b.map(function () { return 0; }) : 0; }
  if (Array.isArray(a) || Array.isArray(b)) {
    a = __vec(a); b = __vec(b);
    return b.map(function (hi, i) { var lo = a[i] || 0; return lo + (hi - lo) * __rand01(); });
  }
  return a + (b - a) * __rand01();
}
function gaussRandom(a, b) {
  // 中心極限定理による近似（12 個の一様乱数の和）
  var s = 0; for (var i = 0; i < 12; i++) s += __rand01();
  var g = (s - 6) / 6 * 0.5 + 0.5;
  if (a === undefined) return g;
  if (b === undefined) { b = a; a = 0; }
  return a + (b - a) * g;
}

// ---- 時間 ----
function timeToFrames(t, fps) {
  if (t === undefined) t = __env().global.time;
  if (fps === undefined) fps = 1 / __env().comp.frameDuration;
  return Math.floor(t * fps + 1e-6);
}
function framesToTime(f, fps) {
  if (fps === undefined) fps = 1 / __env().comp.frameDuration;
  return f / fps;
}
function posterizeTime(fps) {
  var e = __env();
  e.global.time = Math.floor(e.global.time * fps + 1e-6) / fps;
  e.global.value = e.self.valueAtTime(e.global.time);
  return e.global.time;
}

// ---- プロパティ ----
// 値そのもの（配列または Number オブジェクト）に、AE と同じメソッドを付けて返す。
function __makeProp(compId, li, path, self) {
  var e = __env();
  var t = e ? e.global.time : 0;
  var raw = __arr(__eval_prop(compId, li, path, t, !!self));
  var o = Array.isArray(raw) ? raw.slice() : new Number(raw);
  var def = function (name, fn) { Object.defineProperty(o, name, { value: fn, enumerable: false }); };
  var getter = function (name, fn) { Object.defineProperty(o, name, { get: fn, enumerable: false }); };
  getter('value', function () { return Array.isArray(raw) ? raw.slice() : raw; });
  def('valueAtTime', function (tt) { return __arr(__eval_prop(compId, li, path, tt, !!self)); });
  def('velocityAtTime', function (tt) { var h = 0.0005; return div(sub(o.valueAtTime(tt + h), o.valueAtTime(tt - h)), 2 * h); });
  def('speedAtTime', function (tt) { return length(o.velocityAtTime(tt)); });
  getter('velocity', function () { return o.velocityAtTime(__env().global.time); });
  getter('speed', function () { return o.speedAtTime(__env().global.time); });
  var keys = null;
  var keyTimes = function () { if (keys === null) keys = __keys(compId, li, path); return keys; };
  getter('numKeys', function () { return keyTimes().length; });
  def('key', function (i) {
    var k = keyTimes();
    if (i < 1 || i > k.length) throw new RangeError('キー ' + i + ' は範囲外です（1〜' + k.length + '）');
    return { index: i, time: k[i - 1], value: o.valueAtTime(k[i - 1]) };
  });
  def('nearestKey', function (tt) {
    var k = keyTimes();
    if (k.length === 0) throw new RangeError('キーフレームがありません');
    var best = 0;
    for (var i = 1; i < k.length; i++) if (Math.abs(k[i] - tt) < Math.abs(k[best] - tt)) best = i;
    return o.key(best + 1);
  });
  def('wiggle', function (freq, amp, octaves, ampMult, tt) {
    if (octaves === undefined) octaves = 1;
    if (ampMult === undefined) ampMult = 0.5;
    if (tt === undefined) tt = __env().global.time;
    var base = __vec(self ? __env().global.value : o.valueAtTime(tt));
    var seed = __strHash(compId + '/' + li + '/' + path) + (__env().rs.seed | 0) * 7919;
    var out = base.map(function (v, d) {
      var n = 0, a = 1, f = freq, norm = 0;
      for (var oc = 0; oc < Math.max(1, Math.ceil(octaves)); oc++) {
        n += a * __noise1(seed + d * 131 + oc * 17, tt * f);
        norm += a; a *= ampMult; f *= 2;
      }
      return v + amp * n / norm;
    });
    return __arr(out);
  });
  var loop = function (out, type, numKf) {
    var k = keyTimes(), n = k.length, t0 = __env().global.time;
    var current = self ? __env().global.value : o.valueAtTime(t0);
    if (n < 2) return current;
    if (type === undefined) type = 'cycle';
    numKf = numKf | 0;
    var first, last;
    if (out) { last = k[n - 1]; first = numKf > 0 ? k[Math.max(0, n - 1 - numKf)] : k[0]; if (t0 <= last) return current; }
    else { first = k[0]; last = numKf > 0 ? k[Math.min(n - 1, numKf)] : k[n - 1]; if (t0 >= first) return current; }
    var dur = last - first;
    if (dur <= 0) return current;
    var dist = out ? t0 - first : last - t0;
    var cycles = Math.floor(dist / dur + 1e-9), phase = dist - cycles * dur;
    // ちょうど周期の境目では、次の周期の先頭として扱う
    var at = function (p) { return o.valueAtTime(out ? first + p : last - p); };
    if (type === 'cycle') return at(phase);
    if (type === 'pingpong') return cycles % 2 === 1 ? at(dur - phase) : at(phase);
    if (type === 'offset') {
      var delta = out ? sub(o.valueAtTime(last), o.valueAtTime(first)) : sub(o.valueAtTime(first), o.valueAtTime(last));
      return add(at(phase), mul(delta, cycles));
    }
    if (type === 'continue') {
      var edge = out ? last : first, h = 0.001;
      var v = out ? div(sub(o.valueAtTime(edge), o.valueAtTime(edge - h)), h) : div(sub(o.valueAtTime(edge + h), o.valueAtTime(edge)), h);
      return add(o.valueAtTime(edge), mul(v, t0 - edge));
    }
    throw new Error('ループの種類「' + type + '」は未対応です（cycle / pingpong / offset / continue）');
  };
  def('loopOut', function (type, n) { return loop(true, type, n); });
  def('loopIn', function (type, n) { return loop(false, type, n); });
  def('loopOutDuration', function (type, d) { return loop(true, type, 0); });
  def('loopInDuration', function (type, d) { return loop(false, type, 0); });
  def('smooth', function (width, samples, tt) {
    if (width === undefined) width = 0.2;
    if (samples === undefined) samples = 5;
    if (tt === undefined) tt = __env().global.time;
    var acc = null;
    for (var i = 0; i < samples; i++) {
      var v = o.valueAtTime(tt - width / 2 + width * i / Math.max(1, samples - 1));
      acc = acc === null ? v : add(acc, v);
    }
    return div(acc, samples);
  });
  return o;
}

// `with` で使う、プロパティのメソッドだけを集めた入れ物（配列の length などが名前を隠さないようにする）
function __propScope(p) {
  var s = {};
  ['valueAtTime', 'velocityAtTime', 'speedAtTime', 'key', 'nearestKey', 'wiggle', 'loopOut', 'loopIn', 'loopOutDuration', 'loopInDuration', 'smooth'].forEach(function (n) { s[n] = p[n]; });
  ['velocity', 'speed', 'numKeys'].forEach(function (n) { Object.defineProperty(s, n, { get: function () { return p[n]; }, enumerable: true }); });
  return s;
}

function __markerObj(compId, li) {
  var list = JSON.parse(__markers(compId, li));
  var key = function (i) {
    if (typeof i === 'string') { for (var j = 0; j < list.length; j++) if (list[j].comment === i) return key(j + 1); throw new Error('マーカー「' + i + '」が見つかりません'); }
    if (i < 1 || i > list.length) throw new RangeError('マーカー ' + i + ' は範囲外です（1〜' + list.length + '）');
    var m = list[i - 1];
    return { index: i, time: m.time, duration: m.duration, comment: m.comment };
  };
  return {
    numKeys: list.length,
    key: key,
    nearestKey: function (t) {
      if (list.length === 0) throw new RangeError('マーカーがありません');
      var best = 0;
      for (var i = 1; i < list.length; i++) if (Math.abs(list[i].time - t) < Math.abs(list[best].time - t)) best = i;
      return key(best + 1);
    }
  };
}

function __makeLayer(compId, li) {
  var info = JSON.parse(__layer_info(compId, '#' + (li + 1)));
  var prop = function (path) { return __makeProp(compId, li, path, false); };
  var L = {};
  var g = function (name, fn) { Object.defineProperty(L, name, { get: fn, enumerable: true }); };
  L.index = info.index; L.name = info.name; L.inPoint = info.inPoint; L.outPoint = info.outPoint; L.startTime = info.startTime;
  L.width = info.width; L.height = info.height; L.hasParent = info.parent >= 0;
  g('parent', function () { if (info.parent < 0) throw new Error('レイヤー「' + info.name + '」には親がありません'); return __makeLayer(compId, info.parent); });
  var transform = {};
  var tg = function (name, path) { Object.defineProperty(transform, name, { get: function () { return prop(path); }, enumerable: true }); g(name, function () { return prop(path); }); };
  tg('position', 'transform.position'); tg('anchorPoint', 'transform.anchor'); tg('scale', 'transform.scale');
  tg('rotation', 'transform.rotation'); tg('zRotation', 'transform.rotation'); tg('opacity', 'transform.opacity');
  tg('xRotation', 'transform.rotation_x'); tg('yRotation', 'transform.rotation_y');
  L.transform = transform;
  g('timeRemap', function () { return prop('time_remap'); });
  g('audioLevels', function () { return prop('audio.level'); });
  g('marker', function () { return __markerObj(compId, li); });
  L.effect = function (which) {
    var fn = function (param) {
      var path = __fx_path(compId, li, String(which), String(param));
      if (path === '') throw new Error('エフェクト「' + which + '」のパラメータ「' + param + '」が見つかりません');
      if (path.indexOf('layer:') === 0) return __makeLayer(compId, parseInt(path.substring(6), 10));
      if (path.indexOf('const:') === 0) return Number(path.substring(6));
      return prop(path);
    };
    fn.param = fn;
    return fn;
  };
  L.mask = function (i) {
    var idx = typeof i === 'number' ? i - 1 : 0;
    var m = {};
    ['feather', 'opacity', 'expansion'].forEach(function (n) { Object.defineProperty(m, n === 'feather' ? 'maskFeather' : n === 'opacity' ? 'maskOpacity' : 'maskExpansion', { get: function () { return prop('mask.' + idx + '.' + n); } }); });
    return m;
  };
  g('text', function () { return { sourceText: __text(compId, li) }; });
  L.sourceRectAtTime = function (t) {
    if (t === undefined) t = __env().global.time;
    var r = __source_rect(compId, li, t);
    return { left: r[0], top: r[1], width: r[2], height: r[3] };
  };
  L.toComp = function (pt, t) { if (t === undefined) t = __env().global.time; pt = __vec(pt); return __to_comp(compId, li, pt[0] || 0, pt[1] || 0, t, false); };
  L.toWorld = L.toComp;
  L.fromComp = function (pt, t) { if (t === undefined) t = __env().global.time; pt = __vec(pt); return __to_comp(compId, li, pt[0] || 0, pt[1] || 0, t, true); };
  L.fromWorld = L.fromComp;
  return L;
}

function __makeComp(idOrName) {
  var info = JSON.parse(__comp_info(String(idOrName)));
  var C = { name: info.name, width: info.width, height: info.height, duration: info.duration, frameDuration: info.frameDuration, numLayers: info.numLayers, pixelAspect: 1 };
  C.layer = function (which, rel) {
    var idx;
    if (which && typeof which === 'object' && typeof which.index === 'number') idx = which.index + (rel | 0);
    else if (typeof which === 'number' || which instanceof Number) idx = Number(which);
    else {
      var found = JSON.parse(__layer_info(info.id, String(which)));
      if (found.index < 0) throw new Error('レイヤー「' + which + '」が見つかりません');
      idx = found.index;
    }
    if (idx < 1 || idx > info.numLayers) throw new RangeError('レイヤー ' + idx + ' は範囲外です（1〜' + info.numLayers + '）');
    return __makeLayer(info.id, idx - 1);
  };
  Object.defineProperty(C, 'marker', { get: function () { return __markerObj(info.id, -1); } });
  return C;
}
function comp(name) { return __makeComp(name); }

// エクスプレッション用ライブラリ（プラグインの expressions）の関数の中からも、評価中の thisComp などを参照できるようにする
['thisComp', 'thisLayer', 'thisProperty', 'time', 'index'].forEach(function (name) {
  Object.defineProperty(globalThis, name, {
    get: function () { var e = __env(); return e && e.global ? e.global[name] : undefined; },
    configurable: true
  });
});

// ---- 評価の入口 ----
function __evalExpr(__src, compId, li, path, t, value) {
  var self = null;
  var env = { global: {}, rs: { seed: 0, timeless: false, n: 0 }, seed: __strHash(compId + '/' + li + '/' + path), comp: null, self: null };
  env.global.time = t;
  __envStack.push(env);
  try {
    env.comp = __makeComp(compId);
    var layer = __makeLayer(compId, li);
    self = __makeProp(compId, li, path, true);
    env.self = self;
    env.global.value = __arr(value);
    env.global.thisComp = env.comp;
    env.global.thisLayer = layer;
    env.global.thisProperty = self;
    env.global.index = layer.index;
    var __r = __run(__src, layer, __propScope(self), env.global);
    return __unwrap(__r);
  } finally {
    __envStack.pop();
  }
}

function __evalSelector(__src, textIndex, textTotal, selectorValue, t) {
  var env = { global: { time: t, textIndex: textIndex, textTotal: textTotal, selectorValue: selectorValue }, rs: { seed: 0, timeless: false, n: 0 }, seed: textIndex * 7919, comp: { frameDuration: 1 / 30 }, self: null };
  __envStack.push(env);
  try {
    return __unwrap(__run(__src, {}, {}, env.global));
  } finally {
    __envStack.pop();
  }
}
