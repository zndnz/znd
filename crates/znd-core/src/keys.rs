//! 複数のキーフレームをまとめて扱う操作: 範囲選択、コピー＆ペースト、移動、伸縮、反転、補間の一括変更。
//!
//! キーは「プロパティのパス + 時刻」（[`KeyAddr`]）で指す。UI（タイムライン・グラフエディター）は
//! 選択を [`KeyAddr`] の列として持ち、ここの関数で書き換えて、戻り値の新しい選択に置き換える。

use crate::model::Layer;
use crate::property::{Animatable, BezierMode, Ease, Interp, Keyframe, Property};
use crate::shape::BezPath;
use crate::time::{FrameRate, Time};
use crate::visit::{self, PropMut, PropRef};
use serde::{Deserialize, Serialize};

/// レイヤー内の 1 つのキーフレーム。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct KeyAddr {
    pub path: String,
    pub time: Time,
}

impl KeyAddr {
    pub fn new(path: impl Into<String>, time: Time) -> Self {
        KeyAddr { path: path.into(), time }
    }
}

/// コピーしたキー（型ごと）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClipKeys {
    F1(Vec<Keyframe<f64>>),
    F2(Vec<Keyframe<[f64; 2]>>),
    F3(Vec<Keyframe<[f64; 3]>>),
    F4(Vec<Keyframe<[f64; 4]>>),
    Path(Vec<Keyframe<BezPath>>),
}

impl ClipKeys {
    fn times(&self) -> Vec<Time> {
        match self {
            ClipKeys::F1(k) => k.iter().map(|k| k.time).collect(),
            ClipKeys::F2(k) => k.iter().map(|k| k.time).collect(),
            ClipKeys::F3(k) => k.iter().map(|k| k.time).collect(),
            ClipKeys::F4(k) => k.iter().map(|k| k.time).collect(),
            ClipKeys::Path(k) => k.iter().map(|k| k.time).collect(),
        }
    }
}

/// キーのクリップボード。プロパティごとにキーを持ち、時刻は全体の先頭キーからの相対で扱う。
/// レイヤー・コンポをまたいで貼り付けられる（JSON にしてシステムのクリップボードにも置ける）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyClipboard {
    pub entries: Vec<(String, ClipKeys)>,
}

impl KeyClipboard {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 全体の先頭キーの時刻。
    pub fn origin(&self) -> Option<Time> {
        self.entries.iter().flat_map(|(_, k)| k.times()).min()
    }

    pub fn key_count(&self) -> usize {
        self.entries.iter().map(|(_, k)| k.times().len()).sum()
    }

    /// システムのクリップボードに置く文字列。
    pub fn to_text(&self) -> String {
        format!("{CLIP_TAG}{}", serde_json::to_string(self).unwrap_or_default())
    }

    /// [`KeyClipboard::to_text`] で作った文字列から戻す。キーのコピーでなければ `None`。
    pub fn from_text(s: &str) -> Option<Self> {
        serde_json::from_str(s.trim().strip_prefix(CLIP_TAG)?).ok()
    }
}

const CLIP_TAG: &str = "znd-keys:";

// ---------- 型ごとの処理への振り分け ----------

/// `PropMut` の中身の型で `$body` を展開する。`$p` は `&mut Property<V>`。
macro_rules! with_prop {
    ($m:expr, |$p:ident| $body:expr) => {
        match $m {
            PropMut::F1($p) => $body,
            PropMut::F2($p) => $body,
            PropMut::F3($p) => $body,
            PropMut::F4($p) => $body,
            PropMut::Path($p) => $body,
        }
    };
}

fn key_times(r: &PropRef) -> Vec<Time> {
    r.key_times()
}

/// レイヤーのキーをすべて列挙する。
pub fn all_keys(layer: &Layer) -> Vec<KeyAddr> {
    let mut out = Vec::new();
    visit::visit_layer(layer, &mut |path, r| out.extend(key_times(&r).into_iter().map(|t| KeyAddr::new(path, t))));
    out
}

/// 時刻 `t0..=t1` にあるキー。`paths` を指定すると、そのプロパティだけ（前方一致: `transform` で配下すべて）。
pub fn keys_in_range(layer: &Layer, t0: Time, t1: Time, paths: Option<&[String]>) -> Vec<KeyAddr> {
    let (lo, hi) = (t0.min(t1), t0.max(t1));
    all_keys(layer)
        .into_iter()
        .filter(|k| k.time >= lo && k.time <= hi)
        .filter(|k| paths.is_none_or(|ps| ps.iter().any(|p| path_matches(&k.path, p))))
        .collect()
}

/// `path` が `prefix` そのものか、その配下（`prefix.` で始まる）か。
pub fn path_matches(path: &str, prefix: &str) -> bool {
    path == prefix || path.strip_prefix(prefix).is_some_and(|r| r.starts_with('.'))
}

/// 時刻 `t` にある全プロパティのキー（タイムラインの、レイヤーの行の 1 つの菱形）。
pub fn keys_at(layer: &Layer, t: Time) -> Vec<KeyAddr> {
    keys_in_range(layer, t, t, None)
}

/// 選択をプロパティごとにまとめる（時刻は昇順・重複なし）。
fn by_path(sel: &[KeyAddr]) -> Vec<(String, Vec<Time>)> {
    let mut out: Vec<(String, Vec<Time>)> = Vec::new();
    for k in sel {
        match out.iter_mut().find(|(p, _)| *p == k.path) {
            Some((_, ts)) => ts.push(k.time),
            None => out.push((k.path.clone(), vec![k.time])),
        }
    }
    for (_, ts) in &mut out {
        ts.sort();
        ts.dedup();
    }
    out
}

/// `path` のプロパティに `f` を適用する。見つかれば true。
fn edit_prop(layer: &mut Layer, path: &str, f: &mut dyn FnMut(PropMut)) -> bool {
    let mut found = false;
    visit::visit_layer_mut(layer, &mut |p, m| {
        if p == path {
            found = true;
            f(m);
        }
    });
    found
}

fn indices<V: Animatable>(p: &Property<V>, times: &[Time]) -> Vec<usize> {
    times.iter().filter_map(|t| p.key_index_at(*t)).collect()
}

/// 選択キーの時刻を `map` で付け替える。移動先にある選択外のキーは消す（移動してきたキーが勝つ）。
/// 付け替えたあとの時刻を返す。
fn retime<V: Animatable>(p: &mut Property<V>, times: &[Time], map: &dyn Fn(Time) -> Time) -> Vec<Time> {
    let idx = indices(p, times);
    if idx.is_empty() {
        return Vec::new();
    }
    let mut moved: Vec<Keyframe<V>> = Vec::new();
    // 後ろから抜くと番号がずれない
    let mut sorted = idx.clone();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    for i in sorted {
        moved.push(p.keyframes.remove(i));
    }
    let mut out = Vec::new();
    for mut k in moved {
        k.time = map(k.time);
        p.keyframes.retain(|e| e.time != k.time);
        out.push(k.time);
        p.keyframes.push(k);
    }
    p.normalize_keys();
    out.sort();
    out.dedup();
    out
}

// ---------- 操作 ----------

/// 選択キーをコピーする。
pub fn copy(layer: &Layer, sel: &[KeyAddr]) -> KeyClipboard {
    let groups = by_path(sel);
    let mut entries = Vec::new();
    visit::visit_layer(layer, &mut |path, r| {
        let Some((_, times)) = groups.iter().find(|(p, _)| p == path) else { return };
        fn pick<V: Clone>(p: &Property<V>, times: &[Time]) -> Vec<Keyframe<V>> {
            p.keyframes.iter().filter(|k| times.contains(&k.time)).cloned().collect()
        }
        let keys = match r {
            PropRef::F1(p) => ClipKeys::F1(pick(p, times)),
            PropRef::F2(p) => ClipKeys::F2(pick(p, times)),
            PropRef::F3(p) => ClipKeys::F3(pick(p, times)),
            PropRef::F4(p) => ClipKeys::F4(pick(p, times)),
            PropRef::Path(p) => ClipKeys::Path(pick(p, times)),
        };
        if !keys.times().is_empty() {
            entries.push((path.to_string(), keys));
        }
    });
    KeyClipboard { entries }
}

/// 貼り付ける。全体の先頭キーが `at` に来る。
///
/// コピーしたプロパティが 1 つで `target` を指定したときは、型が合えば `target` のプロパティへ貼る
/// （位置のキーを別のレイヤーのアンカーポイントへ、など）。それ以外は同じパスのプロパティへ貼る。
/// 貼り付けたキーを返す。
pub fn paste(layer: &mut Layer, clip: &KeyClipboard, at: Time, target: Option<&str>) -> Vec<KeyAddr> {
    let Some(origin) = clip.origin() else { return Vec::new() };
    let mut out = Vec::new();
    for (src_path, keys) in &clip.entries {
        let dest = match target {
            Some(t) if clip.entries.len() == 1 => t,
            _ => src_path.as_str(),
        };
        // 先頭キーとの差を保つ（プロパティごとの先頭ではなく、全体の先頭）
        let shift = |k_time: Time| at + (k_time - origin);
        edit_prop(layer, dest, &mut |m| {
            fn put<V: Clone>(p: &mut Property<V>, keys: &[Keyframe<V>], shift: &dyn Fn(Time) -> Time, out: &mut Vec<Time>) {
                for k in keys {
                    let mut k = k.clone();
                    k.time = shift(k.time);
                    out.push(k.time);
                    match p.keyframes.binary_search_by_key(&k.time, |e| e.time) {
                        Ok(i) => p.keyframes[i] = k,
                        Err(i) => p.keyframes.insert(i, k),
                    }
                }
            }
            let mut times = Vec::new();
            match (m, keys) {
                (PropMut::F1(p), ClipKeys::F1(k)) => put(p, k, &shift, &mut times),
                (PropMut::F2(p), ClipKeys::F2(k)) => put(p, k, &shift, &mut times),
                (PropMut::F3(p), ClipKeys::F3(k)) => put(p, k, &shift, &mut times),
                (PropMut::F4(p), ClipKeys::F4(k)) => put(p, k, &shift, &mut times),
                (PropMut::Path(p), ClipKeys::Path(k)) => put(p, k, &shift, &mut times),
                // 型が違うプロパティには貼らない
                _ => {}
            }
            out.extend(times.into_iter().map(|t| KeyAddr::new(dest, t)));
        });
    }
    out.sort();
    out.dedup();
    out
}

/// 選択キーを `dt` ずらす。移動先の選択外のキーは置き換わる。新しい選択を返す。
pub fn shift(layer: &mut Layer, sel: &[KeyAddr], dt: Time) -> Vec<KeyAddr> {
    remap(layer, sel, &|t| t + dt)
}

/// 選択キーの時刻を `pivot` を中心に `factor` 倍する（フレーム境界に丸める）。負の倍率は反転になる。
pub fn scale(layer: &mut Layer, sel: &[KeyAddr], pivot: Time, factor: f64, fr: FrameRate) -> Vec<KeyAddr> {
    remap(layer, sel, &|t| {
        let raw = pivot + Time(((t - pivot).0 as f64 * factor).round() as i64);
        crate::beat::nearest_frame(raw, fr)
    })
}

/// 選択キーを時間方向に反転する（選択全体の最初と最後の時刻は保つ）。補間・イーズの入る側と出る側も入れ替える。
pub fn reverse(layer: &mut Layer, sel: &[KeyAddr]) -> Vec<KeyAddr> {
    let Some((lo, hi)) = span(sel) else { return Vec::new() };
    reverse_between(layer, sel, lo, hi)
}

/// 選択キーを `lo..=hi` の範囲で反転する（時刻 t → lo + hi - t）。複数のレイヤーをまとめて反転するときに、全体の範囲を渡す。
pub fn reverse_between(layer: &mut Layer, sel: &[KeyAddr], lo: Time, hi: Time) -> Vec<KeyAddr> {
    let mut out = Vec::new();
    for (path, times) in by_path(sel) {
        edit_prop(layer, &path, &mut |m| {
            fn rev<V: Animatable>(p: &mut Property<V>, times: &[Time], lo: Time, hi: Time) -> Vec<Time> {
                for i in indices(p, times) {
                    let k = &mut p.keyframes[i];
                    std::mem::swap(&mut k.interp_in, &mut k.interp_out);
                    std::mem::swap(&mut k.ease_in, &mut k.ease_out);
                    if let Some(sp) = &mut k.spatial {
                        std::mem::swap(&mut sp.tan_in, &mut sp.tan_out);
                    }
                }
                retime(p, times, &|t| lo + (hi - t))
            }
            let ts = with_prop!(m, |p| rev(p, &times, lo, hi));
            out.extend(ts.into_iter().map(|t| KeyAddr::new(&path, t)));
        });
    }
    out.sort();
    out
}

fn remap(layer: &mut Layer, sel: &[KeyAddr], map: &dyn Fn(Time) -> Time) -> Vec<KeyAddr> {
    let mut out = Vec::new();
    for (path, times) in by_path(sel) {
        edit_prop(layer, &path, &mut |m| {
            let ts = with_prop!(m, |p| retime(p, &times, map));
            out.extend(ts.into_iter().map(|t| KeyAddr::new(&path, t)));
        });
    }
    out.sort();
    out
}

/// 選択キーを消す。消した数を返す。
pub fn delete(layer: &mut Layer, sel: &[KeyAddr]) -> usize {
    let mut n = 0;
    for (path, times) in by_path(sel) {
        edit_prop(layer, &path, &mut |m| {
            with_prop!(m, |p| {
                let before = p.keyframes.len();
                p.keyframes.retain(|k| !times.contains(&k.time));
                n += before - p.keyframes.len();
            })
        });
    }
    n
}

/// キーの補間の一括変更に使う指定。`None` の側は変えない。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyStyle {
    pub interp_in: Option<Interp>,
    pub interp_out: Option<Interp>,
    pub bezier_mode: Option<BezierMode>,
    /// ベジェにする側のイーズ。`None` なら既定（速度 0・影響度 33.3%）。
    pub ease_in: Option<Ease>,
    pub ease_out: Option<Ease>,
}

impl KeyStyle {
    pub const fn new() -> Self {
        KeyStyle { interp_in: None, interp_out: None, bezier_mode: None, ease_in: None, ease_out: None }
    }

    fn apply<V: Animatable>(&self, k: &mut Keyframe<V>, spatial: bool) {
        let dims = if spatial { 1 } else { k.value.dims().max(1) };
        if let Some(i) = self.interp_in {
            k.interp_in = i;
            k.ease_in = match (i, self.ease_in) {
                (Interp::Bezier, Some(e)) => vec![e; dims],
                _ => Vec::new(),
            };
        }
        if let Some(i) = self.interp_out {
            k.interp_out = i;
            // ハンドルで決め直すので、区間のイージングカーブは外す
            k.curve = None;
            k.ease_out = match (i, self.ease_out) {
                (Interp::Bezier, Some(e)) => vec![e; dims],
                _ => Vec::new(),
            };
        }
        if let Some(m) = self.bezier_mode {
            k.bezier_mode = m;
        }
    }
}

impl Default for KeyStyle {
    fn default() -> Self {
        Self::new()
    }
}

/// 選択キーの補間をまとめて変える。変えたキーの数を返す。
pub fn set_style(layer: &mut Layer, sel: &[KeyAddr], style: &KeyStyle) -> usize {
    let mut n = 0;
    for (path, times) in by_path(sel) {
        edit_prop(layer, &path, &mut |m| {
            fn go<V: Animatable>(p: &mut Property<V>, times: &[Time], style: &KeyStyle) -> usize {
                let spatial = p.spatial;
                let idx = indices(p, times);
                for &i in &idx {
                    style.apply(&mut p.keyframes[i], spatial);
                    // 入る側を変えたら、そこへ入る区間のカーブも外す（カーブがハンドルより優先されるため）
                    if style.interp_in.is_some() && i > 0 {
                        p.keyframes[i - 1].curve = None;
                    }
                }
                idx.len()
            }
            n += with_prop!(m, |p| go(p, &times, style));
        });
    }
    n
}

/// 選択キーの区間にイージングカーブを付ける（`None` で外してハンドルに戻す）。付けた区間の数を返す。
///
/// プロパティごとに、選択した隣り合うキーの間の区間に付ける。そのプロパティで 1 つしか選んでいなければ、
/// そのキーから次のキーまでの区間に付ける（最後のキーなら、前のキーからの区間）。
pub fn set_curve(layer: &mut Layer, sel: &[KeyAddr], curve: Option<crate::ease::EaseCurve>) -> usize {
    let mut n = 0;
    for (path, times) in by_path(sel) {
        edit_prop(layer, &path, &mut |m| {
            fn go<V: Animatable>(p: &mut Property<V>, times: &[Time], curve: Option<crate::ease::EaseCurve>) -> usize {
                let segs = segments(p, times);
                for &i in &segs {
                    let k = &mut p.keyframes[i];
                    k.curve = curve.map(|c| c.sanitized());
                    if curve.is_some() {
                        // 停止のままだとカーブが効かない
                        if k.interp_out == Interp::Hold {
                            k.interp_out = Interp::Linear;
                        }
                    }
                }
                segs.len()
            }
            n += with_prop!(m, |p| go(p, &times, curve));
        });
    }
    n
}

/// 選択キーの区間（区間の始めのキーの番号）。`set_curve` と同じ決め方。
fn segments<V: Animatable>(p: &Property<V>, times: &[Time]) -> Vec<usize> {
    let idx = indices(p, times);
    let n = p.keyframes.len();
    if n < 2 {
        return Vec::new();
    }
    if idx.len() == 1 {
        let i = idx[0];
        return vec![if i + 1 < n { i } else { i - 1 }];
    }
    idx.iter().copied().filter(|i| idx.contains(&(i + 1))).collect()
}

/// 選択キーの最初の区間のイージングカーブ（スポイトで拾う用）。区間が無いか、ハンドルで動いていれば `None`。
pub fn curve_of(layer: &Layer, sel: &[KeyAddr]) -> Option<crate::ease::EaseCurve> {
    let mut found = None;
    for (path, times) in by_path(sel) {
        visit::visit_layer(layer, &mut |p, r| {
            if found.is_some() || p != path {
                return;
            }
            fn go<V: Animatable>(p: &Property<V>, times: &[Time]) -> Option<crate::ease::EaseCurve> {
                segments(p, times).first().and_then(|i| p.keyframes[*i].curve)
            }
            found = match r {
                PropRef::F1(p) => go(p, &times),
                PropRef::F2(p) => go(p, &times),
                PropRef::F3(p) => go(p, &times),
                PropRef::F4(p) => go(p, &times),
                PropRef::Path(p) => go(p, &times),
            };
        });
        if found.is_some() {
            break;
        }
    }
    found
}

/// イージングのプリセット。グラフエディターとキーの右クリックメニューに並ぶ。
pub struct EasePreset {
    pub name: &'static str,
    pub style: KeyStyle,
}

const fn bez(influence: f64) -> Option<Ease> {
    Some(Ease { speed: 0.0, influence })
}

const fn both(i: Interp, o: Interp, ease_in: Option<Ease>, ease_out: Option<Ease>) -> KeyStyle {
    KeyStyle { interp_in: Some(i), interp_out: Some(o), bezier_mode: Some(BezierMode::Manual), ease_in, ease_out }
}

pub const EASE_PRESETS: &[EasePreset] = &[
    EasePreset { name: "リニア", style: both(Interp::Linear, Interp::Linear, None, None) },
    EasePreset { name: "イージーイーズ（33%）", style: both(Interp::Bezier, Interp::Bezier, bez(1.0 / 3.0), bez(1.0 / 3.0)) },
    EasePreset { name: "イーズイン（33%）", style: both(Interp::Bezier, Interp::Linear, bez(1.0 / 3.0), None) },
    EasePreset { name: "イーズアウト（33%）", style: both(Interp::Linear, Interp::Bezier, None, bez(1.0 / 3.0)) },
    EasePreset { name: "強いイーズ（75%）", style: both(Interp::Bezier, Interp::Bezier, bez(0.75), bez(0.75)) },
    EasePreset { name: "強いイーズイン（75%）", style: both(Interp::Bezier, Interp::Linear, bez(0.75), None) },
    EasePreset { name: "強いイーズアウト（75%）", style: both(Interp::Linear, Interp::Bezier, None, bez(0.75)) },
    EasePreset { name: "エクスポ風（入 90% / 出 90%）", style: both(Interp::Bezier, Interp::Bezier, bez(0.9), bez(0.9)) },
    EasePreset { name: "穏やか（15%）", style: both(Interp::Bezier, Interp::Bezier, bez(0.15), bez(0.15)) },
    EasePreset {
        name: "自動ベジェ",
        style: KeyStyle {
            interp_in: Some(Interp::Bezier),
            interp_out: Some(Interp::Bezier),
            bezier_mode: Some(BezierMode::Auto),
            ease_in: None,
            ease_out: None,
        },
    },
    EasePreset { name: "停止", style: KeyStyle { interp_out: Some(Interp::Hold), ..KeyStyle::new() } },
];

/// 選択の中で最も早い・遅い時刻。
pub fn span(sel: &[KeyAddr]) -> Option<(Time, Time)> {
    Some((sel.iter().map(|k| k.time).min()?, sel.iter().map(|k| k.time).max()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Layer, LayerKind, Transform};

    fn fr() -> FrameRate {
        FrameRate::FPS_30
    }

    fn f(n: i64) -> Time {
        fr().frame_to_time(n)
    }

    fn layer() -> Layer {
        let mut l = Layer::new("a", "a", LayerKind::Null, Transform::centered(100.0, 100.0, 100.0, 100.0), f(300));
        l.transform.opacity = Property::constant(100.0).with_keys(vec![
            Keyframe::linear(f(0), 0.0),
            Keyframe::linear(f(10), 50.0),
            Keyframe::linear(f(20), 100.0),
        ]);
        l.transform.position =
            Property::spatial([0.0, 0.0]).with_keys(vec![Keyframe::linear(f(0), [0.0, 0.0]), Keyframe::linear(f(30), [100.0, 0.0])]);
        l
    }

    fn times(l: &Layer, path: &str) -> Vec<i64> {
        all_keys(l).into_iter().filter(|k| k.path == path).map(|k| fr().time_to_frame(k.time)).collect()
    }

    #[test]
    fn range_selection_and_prefix() {
        let l = layer();
        assert_eq!(keys_in_range(&l, f(5), f(25), None).len(), 2);
        assert_eq!(keys_at(&l, f(0)).len(), 2);
        let only = keys_in_range(&l, f(0), f(30), Some(&["transform.position".into()]));
        assert_eq!(only.len(), 2);
        assert!(path_matches("transform.position", "transform"));
        assert!(!path_matches("transform.positionx", "transform.position"));
    }

    #[test]
    fn copy_paste_keeps_relative_timing_across_properties() {
        let src = layer();
        let sel = keys_in_range(&src, f(10), f(30), None);
        let clip = copy(&src, &sel);
        assert_eq!(clip.key_count(), 3);
        let clip = KeyClipboard::from_text(&clip.to_text()).unwrap();
        let mut dst = Layer::new("b", "b", LayerKind::Null, Transform::centered(100.0, 100.0, 100.0, 100.0), f(300));
        let pasted = paste(&mut dst, &clip, f(100), None);
        assert_eq!(pasted.len(), 3);
        assert_eq!(times(&dst, "transform.opacity"), vec![100, 110]);
        assert_eq!(times(&dst, "transform.position"), vec![120]);
        assert_eq!(dst.transform.opacity.eval(f(110)), 100.0);
    }

    #[test]
    fn paste_into_other_property_of_same_type() {
        let src = layer();
        let clip = copy(&src, &keys_in_range(&src, f(0), f(30), Some(&["transform.position".into()])));
        let mut dst = layer();
        paste(&mut dst, &clip, f(50), Some("transform.anchor"));
        assert_eq!(times(&dst, "transform.anchor"), vec![50, 80]);
        // 型が違えば貼らない
        let n = paste(&mut dst, &clip, f(50), Some("transform.opacity"));
        assert!(n.is_empty());
    }

    #[test]
    fn shift_overwrites_unselected_destination() {
        let mut l = layer();
        let sel = vec![KeyAddr::new("transform.opacity", f(0))];
        let out = shift(&mut l, &sel, f(10));
        assert_eq!(out, vec![KeyAddr::new("transform.opacity", f(10))]);
        assert_eq!(times(&l, "transform.opacity"), vec![10, 20]);
        assert_eq!(l.transform.opacity.eval(f(10)), 0.0);
    }

    #[test]
    fn shift_selected_block_over_itself() {
        let mut l = layer();
        let sel = keys_in_range(&l, f(0), f(20), Some(&["transform.opacity".into()]));
        shift(&mut l, &sel, f(10));
        assert_eq!(times(&l, "transform.opacity"), vec![10, 20, 30]);
        assert_eq!(l.transform.opacity.eval(f(30)), 100.0);
    }

    #[test]
    fn scale_and_reverse() {
        let mut l = layer();
        let sel = keys_in_range(&l, f(0), f(20), Some(&["transform.opacity".into()]));
        let sel = scale(&mut l, &sel, f(0), 2.0, fr());
        assert_eq!(times(&l, "transform.opacity"), vec![0, 20, 40]);
        let sel = reverse(&mut l, &sel);
        assert_eq!(sel.len(), 3);
        assert_eq!(l.transform.opacity.eval(f(0)), 100.0);
        assert_eq!(l.transform.opacity.eval(f(40)), 0.0);
        // 1/3 倍はフレームに丸まる
        scale(&mut l, &sel, f(0), 1.0 / 3.0, fr());
        assert_eq!(times(&l, "transform.opacity"), vec![0, 7, 13]);
    }

    #[test]
    fn style_and_delete() {
        let mut l = layer();
        let sel = keys_in_range(&l, f(10), f(10), None);
        let preset = EASE_PRESETS.iter().find(|p| p.name.starts_with("強いイーズ（")).unwrap();
        assert_eq!(set_style(&mut l, &sel, &preset.style), 1);
        let k = &l.transform.opacity.keyframes[1];
        assert_eq!((k.interp_in, k.interp_out), (Interp::Bezier, Interp::Bezier));
        assert_eq!(k.ease_in[0].influence, 0.75);
        let every = all_keys(&l);
        assert_eq!(delete(&mut l, &every), 5);
        assert!(all_keys(&l).is_empty());
    }

    #[test]
    fn curves_on_segments() {
        use crate::ease::{EaseCurve, EaseDir};
        let mut l = layer();
        // 隣り合う 2 つを選ぶと、その間の区間に付く
        let sel = keys_in_range(&l, f(0), f(10), Some(&["transform.opacity".into()]));
        let bounce = EaseCurve::bounce(EaseDir::Out);
        assert_eq!(set_curve(&mut l, &sel, Some(bounce)), 1);
        assert_eq!(l.transform.opacity.keyframes[0].curve, Some(bounce));
        assert_eq!(l.transform.opacity.keyframes[1].curve, None);
        assert_eq!(curve_of(&l, &sel), Some(bounce));
        // 値は始点と終点のまま、間がカーブで動く
        let v = l.transform.opacity.eval(f(5));
        assert!((v - 50.0 * bounce.eval(0.5)).abs() < 1e-6, "{v}");
        assert_eq!(l.transform.opacity.eval(f(10)), 50.0);
        // 最後のキーだけ選ぶと、前の区間に付く
        let last = keys_in_range(&l, f(20), f(20), Some(&["transform.opacity".into()]));
        assert_eq!(set_curve(&mut l, &last, Some(EaseCurve::LINEAR)), 1);
        assert_eq!(l.transform.opacity.keyframes[1].curve, Some(EaseCurve::LINEAR));
        // ハンドルの補間を付け直すとカーブは外れる
        let preset = EASE_PRESETS.iter().find(|p| p.name.starts_with("イージーイーズ")).unwrap();
        set_style(&mut l, &last, &preset.style);
        assert_eq!(l.transform.opacity.keyframes[1].curve, None);
        // 空間プロパティでも行き過ぎを表せる（バック アウト）
        let pos = keys_in_range(&l, f(0), f(30), Some(&["transform.position".into()]));
        let back = EaseCurve::bezier(0.34, 1.56, 0.64, 1.0);
        set_curve(&mut l, &pos, Some(back));
        let max_x = (0..=30).map(|i| l.transform.position.eval(f(i))[0]).fold(0.0, f64::max);
        assert!(max_x > 105.0, "{max_x}");
        // 時間反転でカーブも逆になる
        let mut p = l.transform.opacity.clone();
        p.keyframes[0].curve = Some(EaseCurve::elastic(EaseDir::Out));
        p.keyframes[1].curve = None;
        p.reverse_keys(&[0, 1]);
        assert_eq!(p.keyframes[0].curve, Some(EaseCurve::elastic(EaseDir::In)));
        set_curve(&mut l, &sel, None);
        assert_eq!(l.transform.opacity.keyframes[0].curve, None);
    }
}
