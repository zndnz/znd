//! 画面上の直接操作（タイムラインのドラッグ、マスクパス・モーションパスの編集）の中身。
//!
//! UI（znd-app）はポインタの位置をここに渡すだけにして、何がどう変わるかは UI なしでテストできるようにする。

use crate::effect::ParamValue;
use crate::eval::{world_matrix, Affine};
use crate::model::{Composition, Layer, LayerId, LayerKind, Project};
use crate::property::{Property, SpatialTangents};
use crate::shape::{BezPath, Mask, PathVertex};
use crate::time::{FrameRate, Time};
use crate::visit::{self, PropMut};

// ---------- タイムライン ----------

fn for_each_keys(p: &mut PropMut, f: &mut dyn FnMut(&mut Time)) {
    match p {
        PropMut::F1(p) => p.keyframes.iter_mut().for_each(|k| f(&mut k.time)),
        PropMut::F2(p) => p.keyframes.iter_mut().for_each(|k| f(&mut k.time)),
        PropMut::F3(p) => p.keyframes.iter_mut().for_each(|k| f(&mut k.time)),
        PropMut::F4(p) => p.keyframes.iter_mut().for_each(|k| f(&mut k.time)),
        PropMut::Path(p) => p.keyframes.iter_mut().for_each(|k| f(&mut k.time)),
    }
}

fn normalize(p: &mut PropMut) {
    match p {
        PropMut::F1(p) => p.normalize_keys(),
        PropMut::F2(p) => p.normalize_keys(),
        PropMut::F3(p) => p.normalize_keys(),
        PropMut::F4(p) => p.normalize_keys(),
        PropMut::Path(p) => p.normalize_keys(),
    }
}

fn remove_at(p: &mut PropMut, t: Time) -> bool {
    match p {
        PropMut::F1(p) => p.remove_key(t),
        PropMut::F2(p) => p.remove_key(t),
        PropMut::F3(p) => p.remove_key(t),
        PropMut::F4(p) => p.remove_key(t),
        PropMut::Path(p) => p.remove_key(t),
    }
}

/// レイヤーを時間方向に `dt` ずらす。イン・アウト点、開始時刻、すべてのキーフレーム、レイヤーマーカーが一緒に動く。
pub fn shift_layer(layer: &mut Layer, dt: Time) {
    layer.in_point = layer.in_point + dt;
    layer.out_point = layer.out_point + dt;
    layer.start_time = layer.start_time + dt;
    layer.markers.iter_mut().for_each(|m| m.time = m.time + dt);
    visit::visit_layer_mut(layer, &mut |_, mut p| for_each_keys(&mut p, &mut |t| *t = *t + dt));
}

/// イン点を `t` にする（アウト点の 1 フレーム手前まで）。中身の位置は変えない。
pub fn trim_in(layer: &mut Layer, t: Time, fr: FrameRate) {
    let min_len = fr.frame_to_time(1);
    layer.in_point = t.min(layer.out_point - min_len);
}

/// アウト点を `t` にする（イン点の 1 フレーム後ろまで）。
pub fn trim_out(layer: &mut Layer, t: Time, fr: FrameRate) {
    let min_len = fr.frame_to_time(1);
    layer.out_point = t.max(layer.in_point + min_len);
}

/// 時刻 `from` にあるキーフレーム（全プロパティ）を `to` へ動かす。動かした数を返す。
/// 移動先に別のキーがあれば、動かしてきたキーで置き換える。
pub fn move_keys(layer: &mut Layer, from: Time, to: Time) -> usize {
    if from == to {
        return 0;
    }
    let mut moved = 0;
    visit::visit_layer_mut(layer, &mut |_, mut p| {
        let mut here = false;
        for_each_keys(&mut p, &mut |t| here |= *t == from);
        if !here {
            return;
        }
        // 移動先に元からあるキーは消す
        remove_at(&mut p, to);
        for_each_keys(&mut p, &mut |t| {
            if *t == from {
                *t = to;
            }
        });
        normalize(&mut p);
        moved += 1;
    });
    moved
}

/// 時刻 `at` にあるキーフレーム（全プロパティ）を消す。消した数を返す。
pub fn delete_keys(layer: &mut Layer, at: Time) -> usize {
    let mut removed = 0;
    visit::visit_layer_mut(layer, &mut |_, mut p| removed += remove_at(&mut p, at) as usize);
    removed
}

/// スナップ先の候補: ビートグリッド、コンポマーカー、他のレイヤーのイン・アウト点、指定した追加の時刻。
/// `t` から `tolerance` 以内で最も近い候補（フレーム境界に丸めたもの）を返す。無ければ `t` を最寄りフレームに丸める。
pub fn snap_time(comp: &Composition, t: Time, exclude: Option<&LayerId>, extra: &[Time], tolerance: Time) -> Time {
    snap_time_by(comp, t, exclude, extra, tolerance, 1)
}

/// 1 フレームを `subdiv` 等分した刻みのうち、`t` に最も近いもの（`subdiv` が 1 ならフレーム境界）。
/// タイムラインを大きく拡大したときに、フレームより細かく動かすための刻み。
pub fn nearest_step(t: Time, fr: FrameRate, subdiv: u32) -> Time {
    if subdiv <= 1 {
        return crate::beat::nearest_frame(t, fr);
    }
    let f = fr.time_to_frame(t);
    let a = fr.frame_to_time(f);
    let len = (fr.frame_to_time(f + 1) - a).0.max(1);
    let n = subdiv as i64;
    let k = (((t - a).0 as f64 / len as f64) * n as f64).round() as i64;
    a + Time(len * k.clamp(0, n) / n)
}

/// `snap_time` の、丸めの刻みを 1 フレームの `subdiv` 分の 1 にしたもの。
pub fn snap_time_by(comp: &Composition, t: Time, exclude: Option<&LayerId>, extra: &[Time], tolerance: Time, subdiv: u32) -> Time {
    let fr = comp.frame_rate;
    let round = |t: Time| nearest_step(t, fr, subdiv);
    let mut best: Option<(i64, Time)> = None;
    let mut consider = |cand: Time| {
        let cand = round(cand);
        let d = (cand.0 - t.0).abs();
        if d <= tolerance.0 && best.is_none_or(|b| d < b.0) {
            best = Some((d, cand));
        }
    };
    if let Some(g) = &comp.beat_grid {
        consider(g.snap(t));
    }
    comp.markers.iter().for_each(|m| consider(m.time));
    for l in comp.layers.iter().filter(|l| Some(&l.id) != exclude) {
        consider(l.in_point);
        consider(l.out_point);
    }
    extra.iter().for_each(|e| consider(*e));
    best.map(|b| b.1).unwrap_or_else(|| round(t))
}

// ---------- タイムライン: レイヤーの複製・削除・並び替え ----------

/// 複製したレイヤーの名前。末尾の番号を進めて、コンポ内でまだ使われていない名前にする（「平面」→「平面 2」→「平面 3」）。
fn copy_name(comp: &Composition, name: &str) -> String {
    let (base, start) = match name.rsplit_once(' ') {
        Some((base, n)) if !base.is_empty() => match n.parse::<u32>() {
            Ok(n) => (base, n.saturating_add(1)),
            Err(_) => (name, 2),
        },
        _ => (name, 2),
    };
    (start..).map(|n| format!("{base} {n}")).find(|c| comp.layers.iter().all(|l| &l.name != c)).unwrap()
}

/// レイヤーを複製して、元のレイヤーのすぐ上に置く。`new_id` はプロジェクト内で未使用の ID。
pub fn duplicate_layer(comp: &mut Composition, id: &LayerId, new_id: LayerId) -> bool {
    let Some(i) = comp.layer_index(id) else { return false };
    let mut copy = comp.layers[i].clone();
    copy.name = copy_name(comp, &copy.name);
    copy.id = new_id;
    // 同じ時間に重なるので、元の行には入れず、その行のすぐ上に自分だけの行として置く
    copy.track = 0;
    let at = row_range(comp, i).start;
    comp.layers.insert(at, copy);
    true
}

/// レイヤーを時刻 `t` で 2 つに分ける（前半は元のレイヤー、後半は `new_id` で元のすぐ上に置く）。
/// 音MAD の細切れ配置のため、映像も音声もソースの位置はそのまま（後半はソースの続きから始まる）。
/// `t` がイン点・アウト点の間（フレーム境界）に無ければ何もしない。分けたら true。
pub fn split_layer(comp: &mut Composition, id: &LayerId, t: Time, new_id: LayerId) -> bool {
    let Some(i) = comp.layer_index(id) else { return false };
    let t = crate::beat::nearest_frame(t, comp.frame_rate);
    let l = &comp.layers[i];
    if t <= l.in_point || t >= l.out_point {
        return false;
    }
    let mut tail = l.clone();
    tail.name = copy_name(comp, &l.name);
    tail.id = new_id;
    tail.in_point = t;
    tail.markers.retain(|m| m.time >= t);
    // 前半と後半は時間が重ならないので、タイムラインの同じ行に並べる
    tail.track = ensure_track(comp, i);
    let head = &mut comp.layers[i];
    head.out_point = t;
    head.markers.retain(|m| m.time < t);
    comp.layers.insert(i, tail);
    true
}

/// レイヤーを削除する。このレイヤーを指していた親・トラックマット・エフェクトのレイヤー引数は「なし」に戻す。
pub fn remove_layer(comp: &mut Composition, id: &LayerId) -> Option<Layer> {
    let i = comp.layer_index(id)?;
    let removed = comp.layers.remove(i);
    for l in &mut comp.layers {
        if l.parent.as_ref() == Some(id) {
            l.parent = None;
        }
        if l.track_matte.as_ref().is_some_and(|m| &m.layer == id) {
            l.track_matte = None;
        }
        for fx in &mut l.effects {
            for p in fx.params.values_mut() {
                if matches!(p, ParamValue::Layer(Some(target)) if target == id) {
                    *p = ParamValue::Layer(None);
                }
            }
        }
    }
    Some(removed)
}

/// レイヤーを重ね順の `to` 番目の手前へ移す（`to` は移動前の並びでの位置。末尾なら `layers.len()`）。
/// 位置が変わったら true。
pub fn reorder_layer(comp: &mut Composition, id: &LayerId, to: usize) -> bool {
    let Some(from) = comp.layer_index(id) else { return false };
    let to = to.min(comp.layers.len());
    let dest = if to > from { to - 1 } else { to };
    if dest == from {
        return false;
    }
    let layer = comp.layers.remove(from);
    comp.layers.insert(dest, layer);
    true
}

// ---------- タイムライン: 1 行に並ぶレイヤー（トラック） ----------
//
// 重ね順で隣り合い、`track` が同じ 0 以外の番号のレイヤーは、タイムラインの 1 行に時間をずらして並ぶ。
// 1 行の中のレイヤーは時間が重ならないようにしておくので、描画の重ね順は `layers` の並びのままでよい。

/// タイムラインの行（`layers` の添字の範囲、上から順）。
pub fn track_rows(comp: &Composition) -> Vec<std::ops::Range<usize>> {
    let mut rows: Vec<std::ops::Range<usize>> = Vec::new();
    for (i, l) in comp.layers.iter().enumerate() {
        match rows.last_mut() {
            Some(r) if l.track != 0 && comp.layers[r.start].track == l.track => r.end = i + 1,
            _ => rows.push(i..i + 1),
        }
    }
    rows
}

/// `index` 番目のレイヤーがいる行の範囲。
pub fn row_range(comp: &Composition, index: usize) -> std::ops::Range<usize> {
    let track = comp.layers.get(index).map(|l| l.track).unwrap_or(0);
    if track == 0 {
        return index..index + 1;
    }
    let mut a = index;
    while a > 0 && comp.layers[a - 1].track == track {
        a -= 1;
    }
    let mut b = index + 1;
    while b < comp.layers.len() && comp.layers[b].track == track {
        b += 1;
    }
    a..b
}

/// まだ使われていない行の番号。
fn fresh_track(comp: &Composition) -> u32 {
    comp.layers.iter().map(|l| l.track).max().unwrap_or(0) + 1
}

/// `index` 番目のレイヤーに 0 以外の行の番号を付けて、その番号を返す（自分だけの行なら新しい番号）。
pub fn ensure_track(comp: &mut Composition, index: usize) -> u32 {
    if comp.layers[index].track == 0 {
        comp.layers[index].track = fresh_track(comp);
    }
    comp.layers[index].track
}

/// 同じ番号が離れた 2 か所に現れたら、後ろのほうに新しい番号を振る（行どうしが隣り合っても混ざらないように）。
/// 1 つしか入っていない行は 0（自分だけの行）に戻す。
pub fn normalize_tracks(comp: &mut Composition) {
    let rows = track_rows(comp);
    let mut seen = std::collections::HashSet::new();
    let mut next = fresh_track(comp);
    for r in rows {
        let track = comp.layers[r.start].track;
        if r.len() == 1 {
            comp.layers[r.start].track = 0;
        } else if !seen.insert(track) {
            comp.layers[r.clone()].iter_mut().for_each(|l| l.track = next);
            seen.insert(next);
            next += 1;
        }
    }
}

/// レイヤー `id` を、レイヤー `target` のいる行へ移す（その行の先頭に入る）。移したら true。
/// 時間の重なりは直さないので、続けて `trim_row_overlaps` を呼ぶ。
pub fn move_to_row(comp: &mut Composition, id: &LayerId, target: &LayerId) -> bool {
    let (Some(from), Some(to)) = (comp.layer_index(id), comp.layer_index(target)) else { return false };
    if row_range(comp, from).contains(&to) {
        return false;
    }
    let track = ensure_track(comp, to);
    let mut layer = comp.layers.remove(from);
    layer.track = track;
    let start = comp.layer_index(target).map(|i| row_range(comp, i).start).unwrap_or(0);
    comp.layers.insert(start, layer);
    true
}

/// レイヤー `id` を元の行から出して、自分だけの新しい行として重ね順の `to` 番目の手前に置く
/// （`to` は移動前の並びでの位置で、行の境目）。並びか行が変わったら true。
pub fn move_to_new_row(comp: &mut Composition, id: &LayerId, to: usize) -> bool {
    let Some(from) = comp.layer_index(id) else { return false };
    let alone = row_range(comp, from).len() == 1;
    comp.layers[from].track = 0;
    reorder_layer(comp, id, to) || !alone
}

/// 行ごと（その行に並ぶレイヤーをまとめて）重ね順の `to` 番目の手前へ移す（`to` は移動前の並びでの位置で、行の境目）。
/// 位置が変わったら true。
pub fn reorder_row(comp: &mut Composition, id: &LayerId, to: usize) -> bool {
    let Some(i) = comp.layer_index(id) else { return false };
    let r = row_range(comp, i);
    let to = to.min(comp.layers.len());
    if to >= r.start && to <= r.end {
        return false;
    }
    let block: Vec<Layer> = comp.layers.drain(r.clone()).collect();
    let dest = if to > r.start { to - r.len() } else { to };
    comp.layers.splice(dest..dest, block);
    true
}

/// レイヤー `id`（動かしたほう）と同じ行で時間が重なる他のレイヤーを切り詰める。
/// はみ出した側だけを削り、すっかり覆われたものは消し、`id` を両側から挟むものは前後の 2 つに分ける
/// （後ろ半分は `fresh` の ID）。消したレイヤーの ID を返す。
pub fn trim_row_overlaps(comp: &mut Composition, id: &LayerId, fresh: &mut dyn FnMut() -> LayerId) -> Vec<LayerId> {
    let mut removed = Vec::new();
    let Some(i) = comp.layer_index(id) else { return removed };
    let (a, b) = (comp.layers[i].in_point, comp.layers[i].out_point);
    let others: Vec<LayerId> = row_range(comp, i).map(|k| comp.layers[k].id.clone()).filter(|o| o != id).collect();
    for o in others {
        let Some(k) = comp.layer_index(&o) else { continue };
        let l = &comp.layers[k];
        let (oa, ob) = (l.in_point, l.out_point);
        if ob <= a || oa >= b {
            continue;
        }
        if oa >= a && ob <= b {
            remove_layer(comp, &o);
            removed.push(o);
        } else if oa < a && ob > b {
            // 前半は元のレイヤー、後ろ半分は複製（ソースの位置はそのまま）
            let mut tail = l.clone();
            tail.name = copy_name(comp, &l.name);
            tail.id = fresh();
            tail.in_point = b;
            tail.markers.retain(|m| m.time >= b);
            let head = &mut comp.layers[k];
            head.out_point = a;
            head.markers.retain(|m| m.time < a);
            comp.layers.insert(k, tail);
        } else if oa < a {
            comp.layers[k].out_point = a;
        } else {
            comp.layers[k].in_point = b;
        }
    }
    removed
}

// ---------- ビューア: レイヤーの選択と移動 ----------

/// コンポ座標 `pos` にある最前面のレイヤー（寸法を持つレイヤーのみ。ロック・非表示・不透明度 0 は除く）。
pub fn layer_at(project: &Project, comp: &Composition, t: Time, pos: [f64; 2]) -> Option<LayerId> {
    comp.layers
        .iter()
        // 完全に透明なレイヤーは、見えていないので掴めない
        .filter(|l| l.visible && !l.locked && l.active_at(t) && l.transform.opacity.eval(t) > 0.0)
        .find(|l| {
            let Some((w, h)) = project.source_size(&l.kind) else { return false };
            let Some(inv) = world_matrix(comp, l, t).inverse() else { return false };
            let (x, y) = inv.apply(pos[0], pos[1]);
            x >= 0.0 && y >= 0.0 && x <= w && y <= h
        })
        .map(|l| l.id.clone())
}

/// 親の座標系 → コンポ座標（親が無ければ恒等）。
pub fn parent_matrix(comp: &Composition, layer: &Layer, t: Time) -> Affine {
    match layer.parent.as_ref().and_then(|id| comp.layer(id)) {
        Some(parent) => world_matrix(comp, parent, t),
        None => Affine::IDENTITY,
    }
}

/// コンポ座標での移動量 `delta` を、レイヤーの位置プロパティ（親の座標系）の移動量に直す。
pub fn comp_delta_to_parent(comp: &Composition, layer: &Layer, t: Time, delta: [f64; 2]) -> [f64; 2] {
    let m = parent_matrix(comp, layer, t);
    match (Affine { tx: 0.0, ty: 0.0, ..m }).inverse() {
        Some(inv) => {
            let (x, y) = inv.apply(delta[0], delta[1]);
            [x, y]
        }
        None => delta,
    }
}

/// レイヤーをコンポ座標で `delta` だけ動かす。位置がアニメーション中なら、時刻 `t` のキーを動かす（無ければ打つ）。
pub fn move_layer(comp: &mut Composition, id: &LayerId, t: Time, delta: [f64; 2]) {
    let Some(layer) = comp.layer(id) else { return };
    let d = comp_delta_to_parent(comp, layer, t, delta);
    let Some(layer) = comp.layer_mut(id) else { return };
    let p = layer.transform.position.eval(t);
    layer.transform.position.set_value_at(t, [p[0] + d[0], p[1] + d[1]]);
}

// ---------- モーションパス ----------

/// モーションパス上で掴める場所。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MotionHit {
    /// `index` 番目のキーの位置。
    Key(usize),
    TanIn(usize),
    TanOut(usize),
}

/// 位置プロパティのキーの位置（親の座標系）と、接線ハンドルの先端。自動接線のキーはハンドルを持たない。
pub fn motion_handles(pos: &Property<[f64; 2]>) -> Vec<([f64; 2], Option<([f64; 2], [f64; 2])>)> {
    pos.keyframes
        .iter()
        .map(|k| {
            let handles =
                k.spatial.as_ref().filter(|s| !s.auto && s.tan_in.len() >= 2 && s.tan_out.len() >= 2).map(|s| {
                    ([k.value[0] + s.tan_in[0], k.value[1] + s.tan_in[1]], [k.value[0] + s.tan_out[0], k.value[1] + s.tan_out[1]])
                });
            (k.value, handles)
        })
        .collect()
}

/// モーションパスを折れ線にする（親の座標系）。キーが 2 つ未満なら空。
pub fn motion_path_points(pos: &Property<[f64; 2]>, samples_per_segment: usize) -> Vec<[f64; 2]> {
    let n = samples_per_segment.max(1);
    let mut out = Vec::new();
    for w in pos.keyframes.windows(2) {
        let (a, b) = (w[0].time, w[1].time);
        for i in 0..n {
            let t = Time(a.0 + ((b.0 - a.0) as f64 * i as f64 / n as f64) as i64);
            out.push(pos.eval(t));
        }
    }
    if let Some(last) = pos.keyframes.last().filter(|_| pos.keyframes.len() >= 2) {
        out.push(last.value);
    }
    out
}

/// `point`（親の座標系）に最も近い掴みどころ。`radius` は同じ座標系での許容距離。
pub fn motion_hit(pos: &Property<[f64; 2]>, point: [f64; 2], radius: f64) -> Option<MotionHit> {
    let near = |p: [f64; 2]| (p[0] - point[0]).hypot(p[1] - point[1]) <= radius;
    let handles = motion_handles(pos);
    // ハンドルを先に調べる（キーと重なっていたらキーを優先したいので、キーは後から上書きする）
    let mut hit = None;
    for (i, (_, h)) in handles.iter().enumerate() {
        if let Some((tin, tout)) = h {
            if near(*tout) {
                hit = Some(MotionHit::TanOut(i));
            } else if near(*tin) {
                hit = Some(MotionHit::TanIn(i));
            }
        }
    }
    for (i, (p, _)) in handles.iter().enumerate() {
        if near(*p) {
            hit = Some(MotionHit::Key(i));
        }
    }
    hit
}

/// 掴んだ場所を `to`（親の座標系）へ動かす。接線を動かすと、そのキーは手動接線になる。
/// `mirror` が true なら反対側の接線を点対称に保つ（滑らかな頂点）。
pub fn motion_drag(pos: &mut Property<[f64; 2]>, hit: MotionHit, to: [f64; 2], mirror: bool) {
    match hit {
        MotionHit::Key(i) => {
            if let Some(k) = pos.keyframes.get_mut(i) {
                k.value = to;
            }
        }
        MotionHit::TanIn(i) | MotionHit::TanOut(i) => {
            let Some(k) = pos.keyframes.get_mut(i) else { return };
            let v = vec![to[0] - k.value[0], to[1] - k.value[1]];
            let s = k.spatial.get_or_insert(SpatialTangents { tan_in: vec![0.0; 2], tan_out: vec![0.0; 2], auto: false });
            s.auto = false;
            let opposite = vec![-v[0], -v[1]];
            if matches!(hit, MotionHit::TanIn(_)) {
                s.tan_in = v;
                if mirror {
                    s.tan_out = opposite;
                }
            } else {
                s.tan_out = v;
                if mirror {
                    s.tan_in = opposite;
                }
            }
        }
    }
}

/// キーの空間接線を、手動のハンドル（前後のキーへ向かう 1/3 の長さ）に切り替える。ハンドルを表示して掴めるようにする。
pub fn motion_make_manual(pos: &mut Property<[f64; 2]>, index: usize) {
    let n = pos.keyframes.len();
    if index >= n {
        return;
    }
    let cur = pos.keyframes[index].value;
    let prev = if index > 0 { pos.keyframes[index - 1].value } else { cur };
    let next = if index + 1 < n { pos.keyframes[index + 1].value } else { cur };
    // 前後を結ぶ向きに沿った、滑らかな接線
    let dir = [(next[0] - prev[0]) / 6.0, (next[1] - prev[1]) / 6.0];
    pos.keyframes[index].spatial = Some(SpatialTangents { tan_in: vec![-dir[0], -dir[1]], tan_out: vec![dir[0], dir[1]], auto: false });
    pos.spatial = true;
}

// ---------- マスクパス ----------

/// パス上で掴める場所。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathHit {
    Vertex(usize),
    TanIn(usize),
    TanOut(usize),
    /// 区間 `seg` の、パラメータ `t`（0..1）の位置。
    Segment(usize, f64),
}

fn add(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] + b[0], a[1] + b[1]]
}
fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}
fn lerp(a: [f64; 2], b: [f64; 2], t: f64) -> [f64; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}
fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// 3 次ベジェ上の点。
pub fn bezier_point(c: &[[f64; 2]; 4], t: f64) -> [f64; 2] {
    let (q0, q1, q2) = (lerp(c[0], c[1], t), lerp(c[1], c[2], t), lerp(c[2], c[3], t));
    lerp(lerp(q0, q1, t), lerp(q1, q2, t), t)
}

/// パス（レイヤー座標）の、`point` に最も近い掴みどころ。`radius` はレイヤー座標での許容距離。
/// 頂点 → 接線ハンドル → 区間の順に優先する。
pub fn path_hit(path: &BezPath, point: [f64; 2], radius: f64) -> Option<PathHit> {
    let nearest = |candidates: Vec<(PathHit, [f64; 2])>| {
        candidates
            .into_iter()
            .map(|(h, p)| (h, dist(p, point)))
            .filter(|(_, d)| *d <= radius)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(h, _)| h)
    };
    let vertices = path.vertices.iter().enumerate().map(|(i, v)| (PathHit::Vertex(i), v.p)).collect();
    if let Some(h) = nearest(vertices) {
        return Some(h);
    }
    let mut handles = Vec::new();
    for (i, v) in path.vertices.iter().enumerate() {
        if v.tan_in != [0.0; 2] {
            handles.push((PathHit::TanIn(i), add(v.p, v.tan_in)));
        }
        if v.tan_out != [0.0; 2] {
            handles.push((PathHit::TanOut(i), add(v.p, v.tan_out)));
        }
    }
    if let Some(h) = nearest(handles) {
        return Some(h);
    }
    const STEPS: usize = 32;
    let mut best: Option<(PathHit, f64)> = None;
    for seg in 0..path.segment_count() {
        let c = path.segment(seg);
        for i in 1..STEPS {
            let t = i as f64 / STEPS as f64;
            let d = dist(bezier_point(&c, t), point);
            if d <= radius && best.is_none_or(|b| d < b.1) {
                best = Some((PathHit::Segment(seg, t), d));
            }
        }
    }
    best.map(|b| b.0)
}

/// 区間 `seg` の `t` の位置に頂点を足す（形は変わらない）。足した頂点の番号を返す。
pub fn insert_vertex(path: &mut BezPath, seg: usize, t: f64) -> usize {
    let n = path.vertices.len();
    let c = path.segment(seg);
    let (q0, q1, q2) = (lerp(c[0], c[1], t), lerp(c[1], c[2], t), lerp(c[2], c[3], t));
    let (r0, r1) = (lerp(q0, q1, t), lerp(q1, q2, t));
    let s = lerp(r0, r1, t);
    let next = (seg + 1) % n;
    path.vertices[seg].tan_out = sub(q0, c[0]);
    path.vertices[next].tan_in = sub(q2, c[3]);
    let v = PathVertex { p: s, tan_in: sub(r0, s), tan_out: sub(r1, s) };
    path.vertices.insert(seg + 1, v);
    seg + 1
}

/// 掴んだ場所を `to`（レイヤー座標）へ動かす。接線は `mirror` が true なら反対側を点対称に保つ。
/// 区間を掴んだ場合は何もしない（先に `insert_vertex` で頂点にする）。
pub fn path_drag(path: &mut BezPath, hit: PathHit, to: [f64; 2], mirror: bool) {
    match hit {
        PathHit::Vertex(i) => {
            if let Some(v) = path.vertices.get_mut(i) {
                v.p = to;
            }
        }
        PathHit::TanIn(i) => {
            if let Some(v) = path.vertices.get_mut(i) {
                v.tan_in = sub(to, v.p);
                if mirror {
                    v.tan_out = [-v.tan_in[0], -v.tan_in[1]];
                }
            }
        }
        PathHit::TanOut(i) => {
            if let Some(v) = path.vertices.get_mut(i) {
                v.tan_out = sub(to, v.p);
                if mirror {
                    v.tan_in = [-v.tan_out[0], -v.tan_out[1]];
                }
            }
        }
        PathHit::Segment(..) => {}
    }
}

/// 頂点の「角 ⇄ 滑らか」を切り替える。滑らかにするときは、前後の頂点を結ぶ向きの接線を付ける。
pub fn toggle_smooth(path: &mut BezPath, i: usize) {
    let n = path.vertices.len();
    if i >= n {
        return;
    }
    let v = path.vertices[i];
    if v.tan_in != [0.0; 2] || v.tan_out != [0.0; 2] {
        path.vertices[i].tan_in = [0.0; 2];
        path.vertices[i].tan_out = [0.0; 2];
        return;
    }
    let prev = if i > 0 {
        path.vertices[i - 1].p
    } else if path.closed {
        path.vertices[n - 1].p
    } else {
        v.p
    };
    let next = if i + 1 < n {
        path.vertices[i + 1].p
    } else if path.closed {
        path.vertices[0].p
    } else {
        v.p
    };
    let dir = [(next[0] - prev[0]) / 6.0, (next[1] - prev[1]) / 6.0];
    path.vertices[i].tan_in = [-dir[0], -dir[1]];
    path.vertices[i].tan_out = dir;
}

/// マスクパスの値（頂点の位置・接線）を時刻 `t` で編集する。アニメーション中ならその時刻のキーが変わる（無ければ打つ）。
pub fn edit_mask_path(mask: &mut Mask, t: Time, f: impl FnOnce(&mut BezPath)) {
    let mut path = mask.path.eval(t);
    f(&mut path);
    mask.path.set_value_at(t, path);
}

/// マスクパスの構造（頂点の数）を変える編集。キーフレーム間で頂点数がそろっていないと補間できないので、
/// 静的値とすべてのキーに同じ操作を掛ける。
pub fn edit_mask_structure(mask: &mut Mask, f: impl Fn(&mut BezPath)) {
    f(&mut mask.path.value);
    for k in &mut mask.path.keyframes {
        f(&mut k.value);
    }
}

/// 頂点を足す（全キーに同じ位置で足す）。可変幅フェザーの配列もそろえる。
pub fn mask_insert_vertex(mask: &mut Mask, seg: usize, t: f64) -> usize {
    edit_mask_structure(mask, |p| {
        if seg < p.segment_count() {
            insert_vertex(p, seg, t);
        }
    });
    if !mask.vertex_feather.is_empty() {
        let n = mask.vertex_feather.len();
        let (a, b) = (mask.vertex_feather[seg.min(n - 1)], mask.vertex_feather[(seg + 1) % n]);
        mask.vertex_feather.insert((seg + 1).min(n), a + (b - a) * t);
    }
    seg + 1
}

/// 頂点を消す。閉じたパスは 3 頂点、開いたパスは 2 頂点より少なくはしない（消せなければ false）。
pub fn mask_remove_vertex(mask: &mut Mask, i: usize) -> bool {
    let n = mask.path.value.vertices.len();
    let min = if mask.path.value.closed { 3 } else { 2 };
    if i >= n || n <= min {
        return false;
    }
    edit_mask_structure(mask, |p| {
        if i < p.vertices.len() {
            p.vertices.remove(i);
        }
    });
    if i < mask.vertex_feather.len() {
        mask.vertex_feather.remove(i);
    }
    true
}

/// ペンツールで描いている途中のパス。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PenPath {
    pub vertices: Vec<PathVertex>,
}

impl PenPath {
    /// 頂点を足す。`drag_to` があれば、押した位置からそこまで引っ張った滑らかな頂点にする。
    pub fn add(&mut self, p: [f64; 2], drag_to: Option<[f64; 2]>) {
        let out = drag_to.map(|d| sub(d, p)).unwrap_or([0.0; 2]);
        self.vertices.push(PathVertex { p, tan_in: [-out[0], -out[1]], tan_out: out });
    }

    /// 始点の近くをクリックしたか（パスを閉じる合図）。
    pub fn closes_at(&self, p: [f64; 2], radius: f64) -> bool {
        self.vertices.len() >= 3 && dist(self.vertices[0].p, p) <= radius
    }

    /// マスクにする。2 頂点未満では作れない。
    pub fn finish(self, closed: bool) -> Option<Mask> {
        let min = if closed { 3 } else { 2 };
        (self.vertices.len() >= min).then(|| Mask::new(BezPath { vertices: self.vertices, closed }))
    }
}

/// シェイプ・テキストなど寸法を持たないレイヤーかどうか（ビューア上のクリックでは選べない）。
pub fn has_bounds(project: &Project, kind: &LayerKind) -> bool {
    project.source_size(kind).is_some()
}

#[cfg(test)]
mod tests {
    #[test]
    fn split_layer_at_time() {
        use crate::demo::demo_project;
        let mut p = demo_project();
        let comp = &mut p.compositions[0];
        let id = comp.layers[0].id.clone();
        let fr = comp.frame_rate;
        let n = comp.layers.len();
        assert!(!super::split_layer(comp, &id, Time::ZERO, crate::LayerId::new("x")));
        assert!(super::split_layer(comp, &id, fr.frame_to_time(40), crate::LayerId::new("x")));
        assert_eq!(comp.layers.len(), n + 1);
        let (tail, head) = (&comp.layers[0], &comp.layers[1]);
        assert_eq!(tail.id.0, "x");
        assert_eq!(tail.in_point, fr.frame_to_time(40));
        assert_eq!(head.out_point, fr.frame_to_time(40));
        assert_eq!(tail.start_time, head.start_time);
    }

    use super::*;
    use crate::beat::BeatGrid;
    use crate::model::{Marker, Transform};
    use crate::property::Keyframe;
    use crate::shape::MaskMode;

    fn fr() -> FrameRate {
        FrameRate::FPS_30
    }
    fn f(n: i64) -> Time {
        fr().frame_to_time(n)
    }

    fn layer(id: &str) -> Layer {
        let mut l = Layer::new(
            id,
            id,
            LayerKind::Solid { width: 100, height: 50, color: [1.0; 3] },
            Transform::centered(100.0, 50.0, 400.0, 300.0),
            f(60),
        );
        l.in_point = f(10);
        l.out_point = f(40);
        l.transform.opacity = Property::constant(100.0).with_keys(vec![Keyframe::linear(f(10), 0.0), Keyframe::linear(f(20), 100.0)]);
        l.transform.position = Property::spatial([200.0, 150.0])
            .with_keys(vec![Keyframe::linear(f(10), [100.0, 150.0]), Keyframe::linear(f(30), [300.0, 150.0])]);
        l.markers.push(Marker { time: f(15), duration: Time::ZERO, comment: "x".into() });
        l
    }

    fn comp(layers: Vec<Layer>) -> Composition {
        let mut c = Composition::new("main", "main", 400, 300, fr(), f(60));
        c.layers = layers;
        c
    }

    #[test]
    fn shifting_a_layer_moves_keys_and_markers_with_it() {
        let mut l = layer("a");
        shift_layer(&mut l, f(5));
        assert_eq!((l.in_point, l.out_point, l.start_time), (f(15), f(45), f(5)));
        assert_eq!(l.transform.opacity.keyframes.iter().map(|k| k.time).collect::<Vec<_>>(), [f(15), f(25)]);
        assert_eq!(l.transform.position.keyframes[1].time, f(35));
        assert_eq!(l.markers[0].time, f(20));
        // 見た目は「時間だけ」ずれる
        assert!((l.transform.opacity.eval(f(20)) - 50.0).abs() < 1e-9);
        shift_layer(&mut l, f(-5));
        assert_eq!(l, layer("a"));
    }

    #[test]
    fn trimming_keeps_at_least_one_frame() {
        let mut l = layer("a");
        trim_in(&mut l, f(20), fr());
        assert_eq!((l.in_point, l.start_time), (f(20), Time::ZERO));
        trim_in(&mut l, f(99), fr());
        assert_eq!(l.in_point, f(39));
        trim_out(&mut l, f(0), fr());
        assert_eq!(l.out_point, f(40));
        trim_out(&mut l, f(50), fr());
        assert_eq!(l.out_point, f(50));
    }

    #[test]
    fn moving_and_deleting_keys_affects_every_property_at_that_time() {
        let mut l = layer("a");
        // 10f には不透明度と位置の両方にキーがある
        assert_eq!(move_keys(&mut l, f(10), f(12)), 2);
        assert_eq!(l.transform.opacity.keyframes[0].time, f(12));
        assert_eq!(l.transform.position.keyframes[0].time, f(12));
        // 移動先に別のキーがあれば置き換える
        assert_eq!(move_keys(&mut l, f(12), f(20)), 2);
        assert_eq!(l.transform.opacity.keyframes.len(), 1);
        assert_eq!(l.transform.opacity.keyframes[0].value, 0.0);
        assert_eq!(l.transform.position.keyframes.iter().map(|k| k.time).collect::<Vec<_>>(), [f(20), f(30)]);
        assert_eq!(move_keys(&mut l, f(55), f(56)), 0);
        assert_eq!(delete_keys(&mut l, f(20)), 2);
        assert!(!l.transform.opacity.is_animated());
        assert_eq!(l.transform.position.keyframes.len(), 1);
    }

    #[test]
    fn duplicating_a_layer_puts_a_renamed_copy_right_above_it() {
        let mut c = comp(vec![layer("a"), layer("b")]);
        assert!(duplicate_layer(&mut c, &LayerId::new("b"), LayerId::new("b2")));
        assert!(duplicate_layer(&mut c, &LayerId::new("b"), LayerId::new("b3")));
        let names: Vec<_> = c.layers.iter().map(|l| (l.id.0.as_str(), l.name.as_str())).collect();
        assert_eq!(names, [("a", "a"), ("b2", "b 2"), ("b3", "b 3"), ("b", "b")]);
        assert!(!duplicate_layer(&mut c, &LayerId::new("none"), LayerId::new("x")));
    }

    #[test]
    fn removing_a_layer_clears_references_to_it() {
        let mut c = comp(vec![layer("a"), layer("b"), layer("c")]);
        c.layers[0].parent = Some(LayerId::new("b"));
        c.layers[2].track_matte = Some(crate::model::TrackMatte { layer: LayerId::new("b"), mode: crate::model::MatteMode::Alpha });
        c.layers[2].parent = Some(LayerId::new("a"));
        assert_eq!(remove_layer(&mut c, &LayerId::new("b")).map(|l| l.id), Some(LayerId::new("b")));
        assert_eq!(c.layers.len(), 2);
        assert_eq!(c.layers[0].parent, None);
        assert_eq!(c.layers[1].track_matte, None);
        // 関係のない参照は残る
        assert_eq!(c.layers[1].parent, Some(LayerId::new("a")));
        assert!(remove_layer(&mut c, &LayerId::new("b")).is_none());
    }

    #[test]
    fn reordering_moves_a_layer_before_the_given_slot() {
        let order = |c: &Composition| c.layers.iter().map(|l| l.id.0.clone()).collect::<Vec<_>>().join("");
        let mut c = comp(vec![layer("a"), layer("b"), layer("c"), layer("d")]);
        // 自分の前後の隙間に落としても動かない
        assert!(!reorder_layer(&mut c, &LayerId::new("b"), 1));
        assert!(!reorder_layer(&mut c, &LayerId::new("b"), 2));
        assert!(reorder_layer(&mut c, &LayerId::new("a"), 3));
        assert_eq!(order(&c), "bcad");
        assert!(reorder_layer(&mut c, &LayerId::new("d"), 0));
        assert_eq!(order(&c), "dbca");
        assert!(reorder_layer(&mut c, &LayerId::new("d"), 99));
        assert_eq!(order(&c), "bcad");
    }

    fn ids(c: &Composition) -> String {
        c.layers.iter().map(|l| l.id.0.clone()).collect::<Vec<_>>().join("")
    }

    fn rows(c: &Composition) -> String {
        track_rows(c).into_iter().map(|r| c.layers[r].iter().map(|l| l.id.0.clone()).collect::<String>()).collect::<Vec<_>>().join("|")
    }

    fn timed(id: &str, a: i64, b: i64) -> Layer {
        let mut l = layer(id);
        l.in_point = f(a);
        l.out_point = f(b);
        l
    }

    #[test]
    fn neighbours_with_the_same_track_share_a_timeline_row() {
        let mut c = comp(vec![layer("a"), layer("b"), layer("c"), layer("d"), layer("e")]);
        assert_eq!(rows(&c), "a|b|c|d|e");
        c.layers[1].track = 3;
        c.layers[2].track = 3;
        c.layers[4].track = 3;
        // 離れた同じ番号は別の行
        assert_eq!(rows(&c), "a|bc|d|e");
        assert_eq!(row_range(&c, 2), 1..3);
        assert_eq!(row_range(&c, 0), 0..1);
        normalize_tracks(&mut c);
        assert_eq!(c.layers[4].track, 0);
        // 離れた行に同じ番号があったら振り直すので、並び替えて隣り合っても混ざらない
        let mut c = comp(vec![layer("a"), layer("b"), layer("c"), layer("d"), layer("e")]);
        for i in [0, 1, 3, 4] {
            c.layers[i].track = 3;
        }
        assert_eq!(rows(&c), "ab|c|de");
        normalize_tracks(&mut c);
        assert_ne!(c.layers[3].track, 3);
        assert!(reorder_row(&mut c, &LayerId::new("d"), 2));
        assert_eq!(rows(&c), "ab|de|c");
        assert!(reorder_row(&mut c, &LayerId::new("a"), 5));
        assert_eq!(rows(&c), "de|c|ab");
        // 自分の行の中や両端の隙間に落としても動かない
        assert!(!reorder_row(&mut c, &LayerId::new("e"), 1));
        assert!(!reorder_row(&mut c, &LayerId::new("e"), 2));
    }

    #[test]
    fn moving_a_layer_into_another_row_and_back_out() {
        let mut c = comp(vec![timed("a", 0, 10), timed("b", 20, 30), timed("c", 40, 50)]);
        assert!(move_to_row(&mut c, &LayerId::new("c"), &LayerId::new("a")));
        assert_eq!(rows(&c), "ca|b");
        // 同じ行へは移らない
        assert!(!move_to_row(&mut c, &LayerId::new("a"), &LayerId::new("c")));
        assert!(move_to_row(&mut c, &LayerId::new("b"), &LayerId::new("a")));
        assert_eq!(rows(&c), "bca");
        // 行から出して、いちばん下に新しい行として置く
        assert!(move_to_new_row(&mut c, &LayerId::new("c"), 3));
        assert_eq!(rows(&c), "ba|c");
        // 行の真ん中から出して、その場に置いても行は分かれる
        assert!(move_to_row(&mut c, &LayerId::new("c"), &LayerId::new("b")));
        assert_eq!(rows(&c), "cba");
        assert!(move_to_new_row(&mut c, &LayerId::new("b"), 0));
        assert_eq!(rows(&c), "b|ca");
        // 自分だけの行をその場に置き直しても何も変わらない
        assert!(!move_to_new_row(&mut c, &LayerId::new("b"), 1));
    }

    #[test]
    fn overlaps_in_a_row_clip_the_layer_that_was_not_moved() {
        let mut c = comp(vec![timed("m", 15, 25), timed("x", 0, 20), timed("y", 22, 40), timed("z", 16, 24), timed("w", 30, 50)]);
        c.layers.iter_mut().for_each(|l| l.track = 1);
        let mut n = 0;
        let mut fresh = || {
            n += 1;
            LayerId::new(format!("t{n}"))
        };
        let removed = trim_row_overlaps(&mut c, &LayerId::new("m"), &mut fresh);
        // すっかり覆われた z は消え、x は後ろ、y は前を削られる。重ならない w はそのまま
        assert_eq!(removed, vec![LayerId::new("z")]);
        assert_eq!(ids(&c), "mxyw");
        let at = |id: &str| c.layer(&LayerId::new(id)).map(|l| (l.in_point, l.out_point)).unwrap();
        assert_eq!(at("x"), (f(0), f(15)));
        assert_eq!(at("y"), (f(25), f(40)));
        assert_eq!(at("w"), (f(30), f(50)));

        // 動かしたほうを両側から挟むレイヤーは、前後の 2 つに分かれる
        let mut c = comp(vec![timed("m", 20, 30), timed("x", 0, 60)]);
        c.layers.iter_mut().for_each(|l| l.track = 1);
        trim_row_overlaps(&mut c, &LayerId::new("m"), &mut fresh);
        assert_eq!(ids(&c), "mt1x");
        assert_eq!(rows(&c), "mt1x");
        let at = |id: &str| c.layer(&LayerId::new(id)).map(|l| (l.in_point, l.out_point)).unwrap();
        assert_eq!(at("x"), (f(0), f(20)));
        assert_eq!(at("t1"), (f(30), f(60)));

        // 別の行のレイヤーは重なっていても切らない
        let mut c = comp(vec![timed("m", 20, 30), timed("x", 0, 60)]);
        assert!(trim_row_overlaps(&mut c, &LayerId::new("m"), &mut fresh).is_empty());
        assert_eq!(c.layers[1].out_point, f(60));
    }

    #[test]
    fn duplicates_get_their_own_row_and_splits_stay_in_the_row() {
        let mut c = comp(vec![timed("a", 0, 10), timed("b", 20, 30)]);
        c.layers.iter_mut().for_each(|l| l.track = 1);
        assert!(duplicate_layer(&mut c, &LayerId::new("b"), LayerId::new("d")));
        assert_eq!(rows(&c), "d|ab");
        assert!(split_layer(&mut c, &LayerId::new("d"), f(25), LayerId::new("s")));
        assert_eq!(rows(&c), "sd|ab");
    }

    #[test]
    fn steps_finer_than_a_frame() {
        let fr = fr();
        let frame = f(1).0;
        let t = Time(f(10).0 + frame * 3 / 10);
        // 1 フレーム刻みなら 10f、4 等分なら 10f + 1/4
        assert_eq!(nearest_step(t, fr, 1), f(10));
        assert_eq!(nearest_step(t, fr, 4), Time(f(10).0 + frame / 4));
        // 次のフレームの境界にも丸まる
        assert_eq!(nearest_step(Time(f(10).0 + frame * 9 / 10), fr, 4), f(11));
        let c = comp(vec![layer("a")]);
        assert_eq!(snap_time_by(&c, t, None, &[], Time(1), 4), Time(f(10).0 + frame / 4));
    }

    #[test]
    fn snapping_prefers_the_nearest_candidate_within_tolerance() {
        let mut c = comp(vec![layer("a"), layer("b")]);
        c.layers[1].in_point = f(23);
        c.markers.push(Marker { time: f(31), duration: Time::ZERO, comment: String::new() });
        let tol = f(3);
        let id = LayerId::new("a");
        // 他のレイヤーのイン点
        assert_eq!(snap_time(&c, f(22), Some(&id), &[], tol), f(23));
        // 自分自身の端には吸着しない（a のイン点 10f）
        c.layers[1].in_point = f(50);
        assert_eq!(snap_time(&c, f(11), Some(&id), &[], tol), f(11));
        // マーカー
        assert_eq!(snap_time(&c, f(29), Some(&id), &[], tol), f(31));
        // 追加の候補（再生ヘッドなど）
        assert_eq!(snap_time(&c, f(5), Some(&id), &[f(6)], tol), f(6));
        // ビートグリッド: 120BPM = 15 フレームごと
        c.beat_grid = Some(BeatGrid { bpm: 120.0, beats_per_bar: 4, subdivision: 1, offset: Time::ZERO });
        assert_eq!(snap_time(&c, f(16), Some(&id), &[], tol), f(15));
        // 許容範囲の外は、最寄りフレームに丸めるだけ
        assert_eq!(snap_time(&c, Time(f(20).0 + 7), Some(&id), &[], Time(1)), f(20));
    }

    #[test]
    fn viewer_picks_the_topmost_layer_and_moves_it_in_parent_space() {
        let mut top = layer("top");
        top.transform.position = Property::spatial([300.0, 150.0]);
        let mut bottom = layer("bottom");
        bottom.transform.position = Property::spatial([200.0, 150.0]);
        let mut p = Project::new("t");
        p.compositions.push(comp(vec![top, bottom]));
        let c = &p.compositions[0];
        // 重なっている所は上のレイヤー、片方だけの所はそのレイヤー、どちらも無い所は None
        assert_eq!(layer_at(&p, c, f(20), [260.0, 150.0]), Some(LayerId::new("top")));
        assert_eq!(layer_at(&p, c, f(20), [160.0, 150.0]), Some(LayerId::new("bottom")));
        assert_eq!(layer_at(&p, c, f(20), [10.0, 10.0]), None);
        // 表示区間の外では選べない
        assert_eq!(layer_at(&p, c, f(5), [260.0, 150.0]), None);

        // 親が 2 倍に拡大・90° 回転していても、画面上で動かしたぶんだけ動く
        let mut parent = layer("parent");
        parent.transform.position = Property::spatial([200.0, 150.0]);
        parent.transform.scale = Property::constant([200.0, 200.0]);
        parent.transform.rotation = Property::constant(90.0);
        parent.transform.opacity = Property::constant(100.0);
        let mut child = layer("child");
        child.parent = Some(LayerId::new("parent"));
        child.transform.position = Property::spatial([50.0, 25.0]);
        let mut c = comp(vec![child, parent]);
        let id = LayerId::new("child");
        let before = world_matrix(&c, c.layer(&id).unwrap(), f(20)).apply(50.0, 25.0);
        move_layer(&mut c, &id, f(20), [30.0, -10.0]);
        let after = world_matrix(&c, c.layer(&id).unwrap(), f(20)).apply(50.0, 25.0);
        assert!((after.0 - before.0 - 30.0).abs() < 1e-9 && (after.1 - before.1 + 10.0).abs() < 1e-9);
        assert!(!c.layer(&id).unwrap().transform.position.is_animated());
    }

    #[test]
    fn moving_an_animated_layer_edits_or_adds_the_key_at_that_time() {
        let mut c = comp(vec![layer("a")]);
        let id = LayerId::new("a");
        // キーのある時刻: そのキーが動く
        move_layer(&mut c, &id, f(10), [5.0, 7.0]);
        let pos = &c.layer(&id).unwrap().transform.position;
        assert_eq!((pos.keyframes.len(), pos.keyframes[0].value), (2, [105.0, 157.0]));
        // キーの無い時刻: その時刻の値からのキーが打たれる
        move_layer(&mut c, &id, f(20), [0.0, 40.0]);
        let pos = &c.layer(&id).unwrap().transform.position;
        assert_eq!(pos.keyframes.len(), 3);
        assert_eq!(pos.keyframes[1].time, f(20));
        assert!((pos.keyframes[1].value[1] - (157.0 + (150.0 - 157.0) * 0.5 + 40.0)).abs() < 1e-6);
    }

    #[test]
    fn motion_path_keys_and_tangents_can_be_dragged() {
        let mut l = layer("a");
        let pos = &mut l.transform.position;
        let pts = motion_path_points(pos, 8);
        assert_eq!(pts.len(), 9);
        assert_eq!((pts[0], pts[8]), ([100.0, 150.0], [300.0, 150.0]));
        assert_eq!(motion_hit(pos, [301.0, 151.0], 4.0), Some(MotionHit::Key(1)));
        assert_eq!(motion_hit(pos, [200.0, 150.0], 4.0), None);
        motion_drag(pos, MotionHit::Key(1), [300.0, 250.0], false);
        assert_eq!(pos.keyframes[1].value, [300.0, 250.0]);
        // 手動接線に切り替えるとハンドルが現れ、掴める
        assert!(motion_handles(pos)[0].1.is_none());
        motion_make_manual(pos, 0);
        let (tin, tout) = motion_handles(pos)[0].1.unwrap();
        assert!(tout[0] > 100.0 && tin[0] < 100.0);
        assert_eq!(motion_hit(pos, tout, 2.0), Some(MotionHit::TanOut(0)));
        // 接線を上へ引くと、パスの途中が上（y が小さい側）へ膨らむ
        motion_drag(pos, MotionHit::TanOut(0), [100.0, 50.0], true);
        let s = pos.keyframes[0].spatial.clone().unwrap();
        assert_eq!((s.tan_out.clone(), s.tan_in.clone(), s.auto), (vec![0.0, -100.0], vec![0.0, 100.0], false));
        let straight = 150.0 + (250.0 - 150.0) * 0.25;
        assert!(pos.eval(f(15))[1] < straight - 10.0, "{:?}", pos.eval(f(15)));
    }

    #[test]
    fn path_hit_test_prefers_vertices_then_handles_then_segments() {
        let mut path = BezPath::rect(0.0, 0.0, 100.0, 60.0);
        assert_eq!(path_hit(&path, [1.0, 1.0], 4.0), Some(PathHit::Vertex(0)));
        assert_eq!(path_hit(&path, [50.0, 1.0], 4.0), Some(PathHit::Segment(0, 0.5)));
        assert_eq!(path_hit(&path, [50.0, 30.0], 4.0), None);
        path.vertices[0].tan_out = [20.0, -20.0];
        assert_eq!(path_hit(&path, [20.0, -20.0], 4.0), Some(PathHit::TanOut(0)));
        path_drag(&mut path, PathHit::TanOut(0), [30.0, -10.0], true);
        assert_eq!((path.vertices[0].tan_out, path.vertices[0].tan_in), ([30.0, -10.0], [-30.0, 10.0]));
        path_drag(&mut path, PathHit::Vertex(2), [120.0, 80.0], false);
        assert_eq!(path.vertices[2].p, [120.0, 80.0]);
    }

    #[test]
    fn inserting_a_vertex_does_not_change_the_shape() {
        let mut path = BezPath::ellipse([50.0, 50.0], [40.0, 30.0]);
        let before: Vec<[f64; 2]> = (0..=10).map(|i| bezier_point(&path.segment(1), i as f64 / 10.0)).collect();
        let i = insert_vertex(&mut path, 1, 0.3);
        assert_eq!((i, path.vertices.len()), (2, 5));
        // 元の区間 1 は、新しい区間 1（0..0.3）と 2（0.3..1）に分かれた
        for (k, want) in before.iter().enumerate() {
            let t = k as f64 / 10.0;
            let got = if t <= 0.3 { bezier_point(&path.segment(1), t / 0.3) } else { bezier_point(&path.segment(2), (t - 0.3) / 0.7) };
            assert!(dist(got, *want) < 1e-9, "t={t}: {got:?} != {want:?}");
        }
        // 最後の区間（閉じたパスの終点 → 始点）にも足せる
        let n = path.vertices.len();
        insert_vertex(&mut path, n - 1, 0.5);
        assert_eq!(path.vertices.len(), n + 1);
    }

    #[test]
    fn smooth_toggle_and_structural_edits_keep_keyframes_compatible() {
        let mut path = BezPath::rect(0.0, 0.0, 100.0, 60.0);
        toggle_smooth(&mut path, 1);
        assert_eq!(path.vertices[1].tan_out, [-path.vertices[1].tan_in[0], -path.vertices[1].tan_in[1]]);
        assert_ne!(path.vertices[1].tan_out, [0.0; 2]);
        toggle_smooth(&mut path, 1);
        assert_eq!(path.vertices[1].tan_out, [0.0; 2]);

        // アニメーションするマスク: 頂点を足しても、すべてのキーで頂点数がそろう
        let mut mask = Mask::new(BezPath::rect(0.0, 0.0, 100.0, 60.0));
        mask.mode = MaskMode::Add;
        mask.vertex_feather = vec![0.0, 10.0, 20.0, 30.0];
        mask.path = Property::constant(BezPath::rect(0.0, 0.0, 100.0, 60.0)).with_keys(vec![
            Keyframe::linear(f(0), BezPath::rect(0.0, 0.0, 100.0, 60.0)),
            Keyframe::linear(f(10), BezPath::rect(50.0, 0.0, 100.0, 60.0)),
        ]);
        assert_eq!(mask_insert_vertex(&mut mask, 0, 0.5), 1);
        assert!(mask.path.keyframes.iter().all(|k| k.value.vertices.len() == 5));
        assert_eq!(mask.vertex_feather, vec![0.0, 5.0, 10.0, 20.0, 30.0]);
        // 補間できる（中間の時刻で、足した頂点も中間の位置にある）
        assert_eq!(mask.path.eval(f(5)).vertices[1].p, [75.0, 0.0]);
        // 値の編集は、その時刻のキーだけを変える
        edit_mask_path(&mut mask, f(10), |p| path_drag(p, PathHit::Vertex(1), [100.0, -20.0], false));
        assert_eq!(mask.path.keyframes[1].value.vertices[1].p, [100.0, -20.0]);
        assert_eq!(mask.path.keyframes[0].value.vertices[1].p, [50.0, 0.0]);
        // キーの無い時刻で動かすと、キーが打たれる
        edit_mask_path(&mut mask, f(5), |p| path_drag(p, PathHit::Vertex(0), [-5.0, -5.0], false));
        assert_eq!(mask.path.keyframes.len(), 3);
        // 頂点を消す（3 頂点までしか減らせない）
        assert!(mask_remove_vertex(&mut mask, 1));
        assert!(mask_remove_vertex(&mut mask, 0));
        assert!(mask.path.keyframes.iter().all(|k| k.value.vertices.len() == 3));
        assert_eq!(mask.vertex_feather.len(), 3);
        assert!(!mask_remove_vertex(&mut mask, 0));
    }

    #[test]
    fn pen_tool_builds_a_path_click_by_click() {
        let mut pen = PenPath::default();
        pen.add([0.0, 0.0], None);
        pen.add([100.0, 0.0], Some([130.0, 20.0]));
        assert!(!pen.closes_at([1.0, 1.0], 5.0), "2 頂点では閉じられない");
        assert!(pen.clone().finish(true).is_none());
        pen.add([100.0, 80.0], None);
        assert_eq!((pen.vertices[1].tan_out, pen.vertices[1].tan_in), ([30.0, 20.0], [-30.0, -20.0]));
        assert!(pen.closes_at([1.0, 1.0], 5.0));
        assert!(!pen.closes_at([50.0, 50.0], 5.0));
        let mask = pen.finish(true).unwrap();
        assert!(mask.path.value.closed);
        assert_eq!(mask.path.value.vertices.len(), 3);
    }
}
