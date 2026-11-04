// コマンドのスクリプト。plugin.json の "function" に書いた関数が呼ばれる。
// 使えるもの: project（書き換えてよい）, ctx（comp / layer / layers / frame / time / fps）, host, log, comp(), layer(), key() など。

function currentComp() {
  return comp(ctx.comp);
}

function centerSelected() {
  if (!ctx.layer) { host.status('レイヤーを選択してください'); return; }
  var c = currentComp();
  var l = layer(c, ctx.layer);
  var pos = l.transform.position;
  var target = [c.width / 2, c.height / 2];
  if (pos.keyframes && pos.keyframes.length) {
    // キーフレームがあるときは、今の時刻の位置にキーを打つ
    var t = flicks(ctx.time);
    pos.keyframes = pos.keyframes.filter(function (k) { return k.time !== t; });
    pos.keyframes.push(key(ctx.time, target, 'linear'));
    setKeys(pos, pos.keyframes);
  } else {
    pos.value = target;
  }
  host.status('「' + l.name + '」を中央に置きました');
}

function stagger() {
  var c = currentComp();
  var step = flicks(2 / ctx.fps);
  var targets = ctx.layers.length > 1 ? ctx.layers : c.layers.map(function (l) { return l.id; });
  targets.forEach(function (id, i) {
    var l = layer(c, id);
    var d = step * i - (l.in_point - c.layers[0].in_point);
    l.in_point += d; l.start_time += d; l.out_point += d;
  });
  host.status(targets.length + ' 個のレイヤーをずらしました');
}

function report() {
  var c = currentComp();
  log(c.name + ': ' + c.width + 'x' + c.height + ', ' + c.layers.length + ' レイヤー, ' + seconds(c.duration).toFixed(2) + ' 秒');
  c.layers.forEach(function (l, i) { log('  ' + (i + 1) + '. ' + l.name); });
  host.alert(c.name + ' には ' + c.layers.length + ' 個のレイヤーがあります（詳細はログ）');
}
