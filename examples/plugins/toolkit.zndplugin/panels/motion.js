// パネルのスクリプト。
//   panel(state)            → 表示する部品の配列を返す。state はパネルごとに保持される（初回は {}）
//   onEvent(id, value, state) → 部品が操作されたときに呼ばれる。project・state を書き換えてよい
// 部品: label / heading / separator / button / slider / number / checkbox / text / choice / color / row
// それぞれ { type, id, label, value, ... }。row は { type: 'row', items: [...] }。

function defaults(state) {
  if (state.amp == null) state.amp = 20;
  if (state.freq == null) state.freq = 2;
  if (state.axis == null) state.axis = 0;
  return state;
}

function selected() {
  if (!ctx.layer) return null;
  try { return layer(comp(ctx.comp), ctx.layer); } catch (e) { return null; }
}

function panel(state) {
  defaults(state);
  var l = selected();
  var items = [
    { type: 'heading', text: '揺れ（wiggle）' },
    { type: 'label', text: l ? '対象: ' + l.name : 'タイムラインでレイヤーを選んでください' },
    { type: 'slider', id: 'amp', label: '振幅 (px)', min: 0, max: 200, value: state.amp },
    { type: 'slider', id: 'freq', label: '頻度 (回/秒)', min: 0.1, max: 12, value: state.freq },
    { type: 'choice', id: 'axis', label: '方向', options: ['両方', '横だけ', '縦だけ'], value: state.axis },
    { type: 'row', items: [
      { type: 'button', id: 'apply', label: '位置に付ける' },
      { type: 'button', id: 'clear', label: '外す' }
    ] },
    { type: 'separator' },
    { type: 'heading', text: '弾み（ライブラリ関数 bounceOut）' },
    { type: 'button', id: 'bounce', label: 'スケールに弾みを付ける' }
  ];
  if (l && l.transform.position.expression) {
    items.push({ type: 'label', text: '位置のエクスプレッション: ' + l.transform.position.expression.source });
  }
  return items;
}

function onEvent(id, value, state) {
  defaults(state);
  if (id === 'amp' || id === 'freq' || id === 'axis') { state[id] = value; return; }
  var l = selected();
  if (!l) { host.status('レイヤーを選択してください'); return; }
  if (id === 'apply') {
    var w = 'wiggle(' + state.freq + ', ' + state.amp + ')';
    var src = state.axis === 1 ? '[' + w + '[0], value[1]]' : state.axis === 2 ? '[value[0], ' + w + '[1]]' : w;
    l.transform.position.expression = { source: src, enabled: true };
    host.status('「' + l.name + '」の位置に揺れを付けました');
  } else if (id === 'clear') {
    delete l.transform.position.expression;
    delete l.transform.scale.expression;
    host.status('「' + l.name + '」のエクスプレッションを外しました');
  } else if (id === 'bounce') {
    l.transform.scale.expression = { source: 'bounceOut(time - inPoint, value, 0.6)', enabled: true };
    host.status('「' + l.name + '」のスケールに弾みを付けました');
  }
}
