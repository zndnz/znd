// エクスプレッション用ライブラリ。ここで定義した関数は、すべてのエクスプレッションから呼べる。

/** 0 から目標値まで、行き過ぎて戻る弾み（t 秒、duration 秒で落ち着く） */
function bounceOut(t, target, duration) {
  duration = duration || 0.6;
  var u = Math.max(0, Math.min(1, t / duration));
  var k = 1 - Math.cos(u * Math.PI * 2.5) * Math.exp(-u * 5);
  if (Array.isArray(target)) return target.map(function (v) { return v * k; });
  return target * k;
}

/** 一定の周期で値を行き来させる */
function pingPong(t, a, b, period) {
  var u = (t / (period || 1)) % 2;
  if (u > 1) u = 2 - u;
  return linear(u, 0, 1, a, b);
}
