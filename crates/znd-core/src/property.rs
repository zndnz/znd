//! アニメーション可能なプロパティとキーフレーム補間。
//!
//! 時間補間のモデルは After Effects と同じ「速度（単位/秒）＋影響度（0..1）」で、
//! キー間を (時刻, 値) 平面上の 3 次ベジェとして解く。空間プロパティ（位置）は
//! モーションパスの弧長に対して 1 次元のイージングを掛ける。

use crate::time::Time;
use serde::{Deserialize, Serialize};

pub trait Animatable: Clone + PartialEq + std::fmt::Debug {
    /// 成分数。パスのように値ごとに変わる型もある。
    fn dims(&self) -> usize;
    fn component(&self, i: usize) -> f64;
    /// 形（次元数・閉じているか等）は `self` のまま、成分だけ差し替えた値。
    fn with_components(&self, c: &[f64]) -> Self;
    /// `other` との間を補間できるか。できない場合は停止補間になる。
    fn compatible(&self, other: &Self) -> bool {
        self.dims() == other.dims()
    }
}

impl Animatable for f64 {
    fn dims(&self) -> usize {
        1
    }
    fn component(&self, _: usize) -> f64 {
        *self
    }
    fn with_components(&self, c: &[f64]) -> Self {
        c[0]
    }
}

impl<const N: usize> Animatable for [f64; N] {
    fn dims(&self) -> usize {
        N
    }
    fn component(&self, i: usize) -> f64 {
        self[i]
    }
    fn with_components(&self, c: &[f64]) -> Self {
        let mut out = [0.0; N];
        out.copy_from_slice(&c[..N]);
        out
    }
}

/// キーの片側（入る側／出る側）の時間補間。
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interp {
    #[default]
    Linear,
    Bezier,
    /// 停止。出る側に指定すると次のキーまで値を保持する。
    Hold,
}

/// ベジェハンドルの扱い。
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BezierMode {
    /// 入出ハンドルを独立に編集（ベジェ）。
    #[default]
    Manual,
    /// 入出の速度を一致させて編集（連続ベジェ）。評価は Manual と同じ。
    Continuous,
    /// 前後のキーからハンドルを自動計算（自動ベジェ）。保存されたイーズは無視する。
    Auto,
}

#[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ease {
    /// 単位/秒。空間プロパティでは px/秒（パスに沿った速さ）。
    pub speed: f64,
    /// 0..1。キー間の時間幅に対するハンドルの長さ。
    pub influence: f64,
}

impl Ease {
    pub const EASY: Ease = Ease { speed: 0.0, influence: 1.0 / 3.0 };
}

/// モーションパスの接線（キーの値からの相対ベクトル）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpatialTangents {
    pub tan_in: Vec<f64>,
    pub tan_out: Vec<f64>,
    /// true のとき前後のキーから接線を自動計算し、`tan_in`/`tan_out` は無視する。
    #[serde(default)]
    pub auto: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Keyframe<V> {
    pub time: Time,
    pub value: V,
    #[serde(default)]
    pub interp_in: Interp,
    #[serde(default)]
    pub interp_out: Interp,
    /// 次元ごとのイーズ（空間プロパティは 1 要素）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ease_in: Vec<Ease>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ease_out: Vec<Ease>,
    #[serde(default)]
    pub bezier_mode: BezierMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spatial: Option<SpatialTangents>,
    /// ロービング: 前後の固定キーの間で速度が一定になるよう、時刻を自動で決める。
    #[serde(default, skip_serializing_if = "is_false")]
    pub roving: bool,
    /// このキーから次のキーまでの区間のイージングカーブ。あればハンドル（速度・影響度）より優先する。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curve: Option<crate::ease::EaseCurve>,
}

impl<V> Keyframe<V> {
    pub fn linear(time: Time, value: V) -> Self {
        Keyframe {
            time,
            value,
            interp_in: Interp::Linear,
            interp_out: Interp::Linear,
            ease_in: Vec::new(),
            ease_out: Vec::new(),
            bezier_mode: BezierMode::Manual,
            spatial: None,
            roving: false,
            curve: None,
        }
    }

    pub fn hold(time: Time, value: V) -> Self {
        Keyframe { interp_out: Interp::Hold, ..Self::linear(time, value) }
    }

    /// イージーイーズ（速度 0・影響度 33.33%）。
    pub fn easy_ease(time: Time, value: V) -> Self {
        Keyframe { interp_in: Interp::Bezier, interp_out: Interp::Bezier, ..Self::linear(time, value) }
    }

    pub fn auto_bezier(time: Time, value: V) -> Self {
        Keyframe { bezier_mode: BezierMode::Auto, ..Self::easy_ease(time, value) }
    }

    pub fn with_ease(mut self, ease_in: Vec<Ease>, ease_out: Vec<Ease>) -> Self {
        self.ease_in = ease_in;
        self.ease_out = ease_out;
        self
    }

    pub fn with_auto_path(mut self) -> Self {
        self.spatial = Some(SpatialTangents { tan_in: Vec::new(), tan_out: Vec::new(), auto: true });
        self
    }
}

/// エクスプレッション。M1 では保存・読み込みのみで、評価は M3 で実装する。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Expression {
    pub source: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Property<V> {
    /// キーフレームが無いときの値。
    pub value: V,
    /// 時刻昇順。
    #[serde(default = "Vec::new", skip_serializing_if = "Vec::is_empty")]
    pub keyframes: Vec<Keyframe<V>>,
    /// 空間プロパティ（モーションパスを持つ）かどうか。
    #[serde(default, skip_serializing_if = "is_false")]
    pub spatial: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression: Option<Expression>,
}

const AUTO_INFLUENCE: f64 = 1.0 / 6.0;
const LINEAR_INFLUENCE: f64 = 1.0 / 3.0;
const PATH_SAMPLES: usize = 48;

impl<V: Animatable> Property<V> {
    pub fn constant(value: V) -> Self {
        Property { value, keyframes: Vec::new(), spatial: false, expression: None }
    }

    pub fn spatial(value: V) -> Self {
        Property { spatial: true, ..Self::constant(value) }
    }

    pub fn with_keys(mut self, mut keys: Vec<Keyframe<V>>) -> Self {
        keys.sort_by_key(|k| k.time);
        self.keyframes = keys;
        self
    }

    pub fn is_animated(&self) -> bool {
        !self.keyframes.is_empty()
    }

    pub fn key_index_at(&self, t: Time) -> Option<usize> {
        self.keyframes.binary_search_by_key(&t, |k| k.time).ok()
    }

    /// 時刻 `t` にキーを置く（既にあれば値だけ更新）。
    pub fn set_key(&mut self, t: Time, value: V) {
        match self.keyframes.binary_search_by_key(&t, |k| k.time) {
            Ok(i) => self.keyframes[i].value = value,
            Err(i) => self.keyframes.insert(i, Keyframe::linear(t, value)),
        }
    }

    pub fn remove_key(&mut self, t: Time) -> bool {
        match self.key_index_at(t) {
            Some(i) => {
                self.keyframes.remove(i);
                true
            }
            None => false,
        }
    }

    /// UI からの値編集。アニメーション中ならその時刻のキーを、そうでなければ静的値を書き換える。
    pub fn set_value_at(&mut self, t: Time, value: V) {
        if self.is_animated() {
            self.set_key(t, value);
        } else {
            self.value = value;
        }
    }

    /// 時刻 `t` での変化速度（単位/秒、次元ごと）。中心差分で求める。
    pub fn velocity(&self, t: Time) -> Vec<f64> {
        let h = Time(crate::time::FLICKS_PER_SECOND / 2000);
        let (a, b) = (self.eval(t - h), self.eval(t + h));
        let dt = 2.0 * h.seconds();
        (0..a.dims().min(b.dims())).map(|d| (b.component(d) - a.component(d)) / dt).collect()
    }

    /// 選択キー（`indices`）の時刻を `pivot` 基準で `factor` 倍する。負の倍率で反転。
    pub fn scale_keys(&mut self, indices: &[usize], pivot: Time, factor: f64) {
        for &i in indices {
            if let Some(k) = self.keyframes.get_mut(i) {
                k.time = pivot + Time(((k.time - pivot).0 as f64 * factor).round() as i64);
            }
        }
        self.normalize_keys();
    }

    /// 選択キーの時間順を反転する（最初と最後の時刻は保つ）。
    pub fn reverse_keys(&mut self, indices: &[usize]) {
        let times: Vec<Time> = indices.iter().filter_map(|i| self.keyframes.get(*i)).map(|k| k.time).collect();
        let (Some(lo), Some(hi)) = (times.iter().min().copied(), times.iter().max().copied()) else { return };
        // 区間のカーブは区間の始めのキーが持つので、反転後の始めのキーへ時間を逆にして移す
        let n = self.keyframes.len();
        let valid: Vec<usize> = indices.iter().copied().filter(|i| *i < n).collect();
        let mut curves: Vec<(usize, Option<crate::ease::EaseCurve>)> = Vec::new();
        for &j in &valid {
            if valid.contains(&(j + 1)) {
                curves.push((j + 1, self.keyframes[j].curve.map(|c| c.mirrored())));
            }
        }
        if let (Some(first), Some(last)) = (valid.iter().min().copied(), valid.iter().max().copied()) {
            // 最後のキーから外へ出る区間は、反転後に最後になるキー（元の最初のキー）から出る
            curves.push((first, self.keyframes[last].curve));
        }
        for &j in &valid {
            self.keyframes[j].curve = None;
        }
        for (j, c) in curves {
            self.keyframes[j].curve = c;
        }
        for &i in indices {
            if let Some(k) = self.keyframes.get_mut(i) {
                k.time = lo + (hi - k.time);
                std::mem::swap(&mut k.interp_in, &mut k.interp_out);
                std::mem::swap(&mut k.ease_in, &mut k.ease_out);
                if let Some(sp) = &mut k.spatial {
                    std::mem::swap(&mut sp.tan_in, &mut sp.tan_out);
                }
            }
        }
        self.normalize_keys();
    }

    /// キーを時刻順に並べ直し、同時刻の重複は後勝ちで 1 つにする。
    pub fn normalize_keys(&mut self) {
        self.keyframes.sort_by_key(|k| k.time);
        let mut i = 0;
        while i + 1 < self.keyframes.len() {
            if self.keyframes[i].time == self.keyframes[i + 1].time {
                self.keyframes.remove(i);
            } else {
                i += 1;
            }
        }
    }

    /// 別のプロパティからコピーしたキーを、先頭が `at` に来るよう貼り付ける。
    pub fn paste_keys(&mut self, keys: &[Keyframe<V>], at: Time) {
        let Some(first) = keys.iter().map(|k| k.time).min() else { return };
        for k in keys {
            let mut k = k.clone();
            k.time = at + (k.time - first);
            match self.keyframes.binary_search_by_key(&k.time, |e| e.time) {
                Ok(i) => self.keyframes[i] = k,
                Err(i) => self.keyframes.insert(i, k),
            }
        }
    }

    /// ロービング指定のキーの時刻を、前後の固定キーの間で等速になるよう振り直す（空間プロパティのみ）。
    pub fn apply_roving(&mut self) {
        if !self.spatial {
            return;
        }
        let n = self.keyframes.len();
        let mut i = 0;
        while i + 1 < n {
            // i = 固定キー。次の固定キー j を探す
            let mut j = i + 1;
            while j + 1 < n && self.keyframes[j].roving {
                j += 1;
            }
            if j > i + 1 {
                let lens: Vec<f64> = (i..j).map(|s| self.path(s).length()).collect();
                let total: f64 = lens.iter().sum();
                let (t0, t1) = (self.keyframes[i].time, self.keyframes[j].time);
                let mut acc = 0.0;
                for (s, len) in lens.iter().enumerate().take(j - i - 1) {
                    acc += len;
                    let f = if total > 0.0 { acc / total } else { (s + 1) as f64 / (j - i) as f64 };
                    self.keyframes[i + 1 + s].time = t0 + Time(((t1 - t0).0 as f64 * f).round() as i64);
                }
            }
            i = j;
        }
    }

    pub fn eval(&self, t: Time) -> V {
        let keys = &self.keyframes;
        let (first, last) = match (keys.first(), keys.last()) {
            (Some(f), Some(l)) => (f, l),
            _ => return self.value.clone(),
        };
        if t <= first.time {
            return first.value.clone();
        }
        if t >= last.time {
            return last.value.clone();
        }
        // first.time < t < last.time なので 1 <= i <= len-1
        let i = keys.partition_point(|k| k.time <= t) - 1;
        self.eval_segment(i, t)
    }

    fn eval_segment(&self, i: usize, t: Time) -> V {
        let k0 = &self.keyframes[i];
        let k1 = &self.keyframes[i + 1];
        if k0.interp_out == Interp::Hold || !k0.value.compatible(&k1.value) {
            return k0.value.clone();
        }
        let dims = k0.value.dims();
        let dt = (k1.time - k0.time).seconds();
        let tau = (t - k0.time).seconds();
        if let Some(curve) = &k0.curve {
            return self.eval_curve(i, curve.eval(tau / dt));
        }
        let both_linear = k0.interp_out == Interp::Linear && k1.interp_in != Interp::Bezier;

        if self.spatial {
            let path = self.path(i);
            let len = path.length();
            let s = if both_linear {
                len * tau / dt
            } else {
                let e_out = self.ease(i, Side::Out, 0, 0.0, len, dt);
                let e_in = self.ease(i + 1, Side::In, 0, 0.0, len, dt);
                ease_value(0.0, len, dt, tau, e_out, e_in).clamp(0.0, len)
            };
            return k0.value.with_components(&path.point_at_length(s));
        }

        let mut out = vec![0.0f64; dims];
        for (d, o) in out.iter_mut().enumerate() {
            let v0 = k0.value.component(d);
            let v1 = k1.value.component(d);
            *o = if both_linear {
                v0 + (v1 - v0) * tau / dt
            } else {
                let e_out = self.ease(i, Side::Out, d, v0, v1, dt);
                let e_in = self.ease(i + 1, Side::In, d, v0, v1, dt);
                ease_value(v0, v1, dt, tau, e_out, e_in)
            };
        }
        k0.value.with_components(&out)
    }

    /// 区間 `i` を進み具合 `f`（0..1 を外れてもよい）で評価する（イージングカーブ用）。
    fn eval_curve(&self, i: usize, f: f64) -> V {
        let k0 = &self.keyframes[i];
        let k1 = &self.keyframes[i + 1];
        if self.spatial {
            let path = self.path(i);
            let len = path.length();
            let s = len * f;
            if (0.0..=len).contains(&s) || len <= 1e-9 {
                return k0.value.with_components(&path.point_at_length(s.clamp(0.0, len)));
            }
            // 行き過ぎ・戻りは、端の向きに沿って延ばす
            let eps = (len * 1e-3).max(1e-6);
            let (base, near, over) = if s < 0.0 { (0.0, eps, s) } else { (len, len - eps, s - len) };
            let a = path.point_at_length(base);
            let b = path.point_at_length(near);
            let sign = if s < 0.0 { 1.0 } else { -1.0 };
            let c: Vec<f64> = a.iter().zip(&b).map(|(a, b)| a + (b - a) / eps * over * sign).collect();
            return k0.value.with_components(&c);
        }
        let c: Vec<f64> = (0..k0.value.dims()).map(|d| {
            let (v0, v1) = (k0.value.component(d), k1.value.component(d));
            v0 + (v1 - v0) * f
        }).collect();
        k0.value.with_components(&c)
    }

    /// キー `i` の `side` 側の実効イーズ。`v0`,`v1`,`dt` はそのイーズが属する区間の端点値と時間幅。
    fn ease(&self, i: usize, side: Side, d: usize, v0: f64, v1: f64, dt: f64) -> Ease {
        let k = &self.keyframes[i];
        let (interp, stored) = match side {
            Side::In => (k.interp_in, &k.ease_in),
            Side::Out => (k.interp_out, &k.ease_out),
        };
        match interp {
            // ベジェと混在する直線側は、区間の平均速度を向いたハンドルとして扱う。
            Interp::Linear | Interp::Hold => Ease { speed: (v1 - v0) / dt, influence: LINEAR_INFLUENCE },
            Interp::Bezier if k.bezier_mode == BezierMode::Auto => Ease { speed: self.auto_speed(i, d), influence: AUTO_INFLUENCE },
            Interp::Bezier => {
                let e = stored.get(d).or(stored.first()).copied().unwrap_or(Ease::EASY);
                Ease { speed: e.speed, influence: e.influence.clamp(0.001, 1.0) }
            }
        }
    }

    /// 自動ベジェの速度: 前後のキーを結ぶ傾き。端のキーは隣接区間の傾き。
    fn auto_speed(&self, i: usize, d: usize) -> f64 {
        let keys = &self.keyframes;
        let lo = i.saturating_sub(1);
        let hi = (i + 1).min(keys.len() - 1);
        let dt = (keys[hi].time - keys[lo].time).seconds();
        if dt <= 0.0 {
            return 0.0;
        }
        if self.spatial {
            let dist: f64 = (lo..hi).map(|j| self.path(j).length()).sum();
            dist / dt
        } else {
            (keys[hi].value.component(d) - keys[lo].value.component(d)) / dt
        }
    }

    fn tangent(&self, i: usize, side: Side) -> Vec<f64> {
        let keys = &self.keyframes;
        let dims = keys[i].value.dims();
        let zero = vec![0.0; dims];
        let Some(sp) = &keys[i].spatial else {
            return zero;
        };
        if sp.auto {
            if i == 0 || i + 1 == keys.len() {
                return zero;
            }
            let sign = if side == Side::Out { 1.0 } else { -1.0 };
            return (0..dims).map(|d| sign * (keys[i + 1].value.component(d) - keys[i - 1].value.component(d)) / 6.0).collect();
        }
        let t = if side == Side::Out { &sp.tan_out } else { &sp.tan_in };
        if t.len() == dims {
            t.clone()
        } else {
            zero
        }
    }

    /// キー `i`→`i+1` のモーションパス。
    fn path(&self, i: usize) -> SpatialPath {
        let k0 = &self.keyframes[i];
        let k1 = &self.keyframes[i + 1];
        let p0: Vec<f64> = (0..k0.value.dims()).map(|d| k0.value.component(d)).collect();
        let p3: Vec<f64> = (0..k1.value.dims()).map(|d| k1.value.component(d)).collect();
        let t_out = self.tangent(i, Side::Out);
        let t_in = self.tangent(i + 1, Side::In);
        let p1 = p0.iter().zip(&t_out).map(|(p, t)| p + t).collect();
        let p2 = p3.iter().zip(&t_in).map(|(p, t)| p + t).collect();
        let straight = t_out.iter().chain(&t_in).all(|v| *v == 0.0);
        SpatialPath::new([p0, p1, p2, p3], straight)
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Side {
    In,
    Out,
}

/// (0,v0)-(dt,v1) を結ぶ 3 次ベジェ値グラフの、時刻 `tau` での値。
fn ease_value(v0: f64, v1: f64, dt: f64, tau: f64, out: Ease, inn: Ease) -> f64 {
    let x1 = out.influence * dt;
    let x2 = dt - inn.influence * dt;
    let y1 = v0 + out.speed * x1;
    let y2 = v1 - inn.speed * inn.influence * dt;
    // x(s) は影響度が 0..1 の範囲なら単調増加なので二分法で解ける。
    let bez = |a: f64, b: f64, c: f64, d: f64, s: f64| {
        let m = 1.0 - s;
        m * m * m * a + 3.0 * m * m * s * b + 3.0 * m * s * s * c + s * s * s * d
    };
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..48 {
        let mid = 0.5 * (lo + hi);
        if bez(0.0, x1, x2, dt, mid) < tau {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bez(v0, y1, y2, v1, 0.5 * (lo + hi))
}

struct SpatialPath {
    ctrl: [Vec<f64>; 4],
    straight: bool,
    /// 等パラメータ間隔でサンプルした累積弧長。
    cum: Vec<f64>,
}

impl SpatialPath {
    fn new(ctrl: [Vec<f64>; 4], straight: bool) -> Self {
        let mut p = SpatialPath { ctrl, straight, cum: Vec::new() };
        if straight {
            let d = dist(&p.ctrl[0], &p.ctrl[3]);
            p.cum = vec![0.0, d];
        } else {
            let mut cum = Vec::with_capacity(PATH_SAMPLES + 1);
            let mut prev = p.point(0.0);
            let mut acc = 0.0;
            cum.push(0.0);
            for j in 1..=PATH_SAMPLES {
                let cur = p.point(j as f64 / PATH_SAMPLES as f64);
                acc += dist(&prev, &cur);
                cum.push(acc);
                prev = cur;
            }
            p.cum = cum;
        }
        p
    }

    fn length(&self) -> f64 {
        *self.cum.last().unwrap()
    }

    fn point(&self, s: f64) -> Vec<f64> {
        let m = 1.0 - s;
        let [a, b, c, d] = &self.ctrl;
        (0..a.len()).map(|i| m * m * m * a[i] + 3.0 * m * m * s * b[i] + 3.0 * m * s * s * c[i] + s * s * s * d[i]).collect()
    }

    fn point_at_length(&self, len: f64) -> Vec<f64> {
        let total = self.length();
        if total <= 0.0 {
            return self.ctrl[0].clone();
        }
        if self.straight {
            let u = len / total;
            return self.ctrl[0].iter().zip(&self.ctrl[3]).map(|(a, b)| a + (b - a) * u).collect();
        }
        let j = self.cum.partition_point(|c| *c < len).clamp(1, PATH_SAMPLES);
        let (c0, c1) = (self.cum[j - 1], self.cum[j]);
        let f = if c1 > c0 { (len - c0) / (c1 - c0) } else { 0.0 };
        self.point((j as f64 - 1.0 + f) / PATH_SAMPLES as f64)
    }
}

fn dist(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum::<f64>().sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(sec: f64) -> Time {
        Time::from_seconds(sec)
    }

    #[test]
    fn constant_and_clamping() {
        let p = Property::constant(5.0);
        assert_eq!(p.eval(s(3.0)), 5.0);
        let p = Property::constant(0.0).with_keys(vec![Keyframe::linear(s(1.0), 10.0), Keyframe::linear(s(2.0), 20.0)]);
        assert_eq!(p.eval(s(0.0)), 10.0);
        assert_eq!(p.eval(s(5.0)), 20.0);
        assert!((p.eval(s(1.25)) - 12.5).abs() < 1e-9);
    }

    #[test]
    fn hold_keeps_value_until_next_key() {
        let p = Property::constant(0.0).with_keys(vec![Keyframe::hold(s(0.0), 1.0), Keyframe::linear(s(1.0), 2.0)]);
        assert_eq!(p.eval(s(0.999)), 1.0);
        assert_eq!(p.eval(s(1.0)), 2.0);
    }

    #[test]
    fn easy_ease_is_symmetric_and_monotonic() {
        let p = Property::constant(0.0).with_keys(vec![Keyframe::easy_ease(s(0.0), 0.0), Keyframe::easy_ease(s(2.0), 100.0)]);
        assert!((p.eval(s(1.0)) - 50.0).abs() < 1e-6);
        // 速度 0・影響度 1/3 のベジェは x(s) が線形になるので 3s^2-2s^3 と一致する
        let u: f64 = 0.25;
        let expect = 100.0 * (3.0 * u * u - 2.0 * u * u * u);
        assert!((p.eval(s(0.5)) - expect).abs() < 1e-6);
        let mut prev = -1.0;
        for i in 0..=200 {
            let v = p.eval(s(i as f64 * 0.01));
            assert!(v >= prev);
            prev = v;
        }
    }

    #[test]
    fn linear_bezier_with_matching_speed_is_linear() {
        let e = vec![Ease { speed: 50.0, influence: 0.4 }];
        let p = Property::constant(0.0).with_keys(vec![
            Keyframe::easy_ease(s(0.0), 0.0).with_ease(e.clone(), e.clone()),
            Keyframe::easy_ease(s(2.0), 100.0).with_ease(e.clone(), e),
        ]);
        for i in 0..=20 {
            let t = i as f64 * 0.1;
            assert!((p.eval(s(t)) - 50.0 * t).abs() < 1e-6);
        }
    }

    #[test]
    fn auto_bezier_passes_through_keys_smoothly() {
        let p = Property::constant(0.0).with_keys(vec![
            Keyframe::auto_bezier(s(0.0), 0.0),
            Keyframe::auto_bezier(s(1.0), 10.0),
            Keyframe::auto_bezier(s(2.0), 0.0),
        ]);
        assert_eq!(p.eval(s(1.0)), 10.0);
        // 中央キーは前後が同値なので速度 0 → 頂点付近でオーバーシュートしない
        assert!(p.eval(s(0.95)) <= 10.0 + 1e-9);
        assert!(p.eval(s(1.05)) <= 10.0 + 1e-9);
    }

    #[test]
    fn multi_dim_linear() {
        let p =
            Property::constant([0.0, 0.0]).with_keys(vec![Keyframe::linear(s(0.0), [0.0, 10.0]), Keyframe::linear(s(1.0), [100.0, 30.0])]);
        let v = p.eval(s(0.5));
        assert!((v[0] - 50.0).abs() < 1e-9 && (v[1] - 20.0).abs() < 1e-9);
    }

    #[test]
    fn spatial_linear_moves_at_constant_speed_along_curve() {
        let mut k0 = Keyframe::linear(s(0.0), [0.0, 0.0]);
        k0.spatial = Some(SpatialTangents { tan_in: vec![0.0, 0.0], tan_out: vec![0.0, 100.0], auto: false });
        let mut k1 = Keyframe::linear(s(1.0), [100.0, 0.0]);
        k1.spatial = Some(SpatialTangents { tan_in: vec![0.0, 100.0], tan_out: vec![0.0, 0.0], auto: false });
        let p = Property::spatial([0.0, 0.0]).with_keys(vec![k0, k1]);
        // 対称なパスなので中間時刻で x=50、y は膨らんでいる
        let mid = p.eval(s(0.5));
        assert!((mid[0] - 50.0).abs() < 0.5);
        assert!((mid[1] - 75.0).abs() < 0.5);
        // 等時間ステップでの移動距離がほぼ一定
        let mut prev = p.eval(s(0.0));
        let mut steps = Vec::new();
        for i in 1..=10 {
            let cur = p.eval(s(i as f64 * 0.1));
            steps.push(((cur[0] - prev[0]).powi(2) + (cur[1] - prev[1]).powi(2)).sqrt());
            prev = cur;
        }
        let (min, max) = steps.iter().fold((f64::MAX, 0.0f64), |(a, b), v| (a.min(*v), b.max(*v)));
        assert!(max / min < 1.05, "steps {steps:?}");
    }

    #[test]
    fn key_operations() {
        let mut p = Property::constant(0.0).with_keys(vec![
            Keyframe::linear(s(1.0), 1.0),
            Keyframe::linear(s(2.0), 2.0),
            Keyframe::linear(s(4.0), 4.0),
        ]);
        p.scale_keys(&[0, 1, 2], s(1.0), 0.5);
        assert_eq!(p.keyframes.iter().map(|k| k.time).collect::<Vec<_>>(), vec![s(1.0), s(1.5), s(2.5)]);
        p.reverse_keys(&[0, 1, 2]);
        assert_eq!(p.keyframes.iter().map(|k| k.value).collect::<Vec<_>>(), vec![4.0, 2.0, 1.0]);
        assert_eq!(p.keyframes[1].time, s(2.0));
        let copied = p.keyframes.clone();
        let mut q = Property::constant(0.0);
        q.paste_keys(&copied, s(10.0));
        assert_eq!(q.keyframes[0].time, s(10.0));
        assert_eq!(q.keyframes[2].time, s(11.5));
        assert!((p.velocity(s(1.5))[0] + 2.0).abs() < 1e-6);
    }

    #[test]
    fn roving_key_gives_constant_speed() {
        let mut mid = Keyframe::linear(s(0.2), [30.0, 0.0]);
        mid.roving = true;
        let mut p = Property::spatial([0.0, 0.0]).with_keys(vec![
            Keyframe::linear(s(0.0), [0.0, 0.0]),
            mid,
            Keyframe::linear(s(1.0), [100.0, 0.0]),
        ]);
        p.apply_roving();
        assert_eq!(p.keyframes[1].time, s(0.3));
    }

    #[test]
    fn set_value_at_edits_key_or_static() {
        let mut p = Property::constant(1.0);
        p.set_value_at(s(1.0), 2.0);
        assert_eq!(p.value, 2.0);
        assert!(!p.is_animated());
        p.set_key(s(0.0), 0.0);
        p.set_value_at(s(1.0), 9.0);
        assert_eq!(p.keyframes.len(), 2);
        assert_eq!(p.eval(s(1.0)), 9.0);
        assert!(p.remove_key(s(1.0)));
        assert!(!p.remove_key(s(1.0)));
    }
}
