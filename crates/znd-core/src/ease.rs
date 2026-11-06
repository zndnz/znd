//! キーフレーム区間のイージングカーブ（0..1 の時間 → 0..1 の進み具合）。
//!
//! キーの `curve` に置くと、そのキーから次のキーまでの区間を、速度・影響度のハンドルの代わりにこのカーブで動かす。
//! 区間の始点と終点の値はそのままで、間の進み方だけが変わる（値を変えても形は崩れない）。
//! 3 次ベジェ（CSS の `cubic-bezier` と同じ 4 つの数）に加えて、1 本のベジェでは表せない
//! 振動（エラスティック）・バウンス・階段も持てる。

use serde::{Deserialize, Serialize};

/// 振動・バウンスの向き。
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EaseDir {
    /// 始まりで振れる。
    In,
    /// 終わりで振れる（着地してから揺れる）。
    #[default]
    Out,
    /// 両端で振れる。
    InOut,
}

impl EaseDir {
    pub const ALL: [EaseDir; 3] = [EaseDir::In, EaseDir::Out, EaseDir::InOut];

    pub fn label(self) -> &'static str {
        match self {
            EaseDir::In => "イン",
            EaseDir::Out => "アウト",
            EaseDir::InOut => "イン・アウト",
        }
    }

    fn flipped(self) -> Self {
        match self {
            EaseDir::In => EaseDir::Out,
            EaseDir::Out => EaseDir::In,
            EaseDir::InOut => EaseDir::InOut,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EaseCurve {
    /// 3 次ベジェ。(0,0)・(x1,y1)・(x2,y2)・(1,1)。x は 0..1、y は範囲外（行き過ぎ・戻り）も可。
    Bezier { x1: f64, y1: f64, x2: f64, y2: f64 },
    /// 振動。`amplitude` は行き過ぎの強さ（1 が標準）、`frequency` は区間内の振動の回数、`decay` は減衰の速さ。
    Elastic { dir: EaseDir, amplitude: f64, frequency: f64, decay: f64 },
    /// バウンス。`bounces` 回跳ね、跳ねるたびに高さが `restitution`² 倍になる（反発係数）。
    Bounce { dir: EaseDir, bounces: u32, restitution: f64 },
    /// 階段（`count` 段で値が飛ぶ）。
    Steps { count: u32 },
}

impl EaseCurve {
    pub const LINEAR: EaseCurve = EaseCurve::Bezier { x1: 0.0, y1: 0.0, x2: 1.0, y2: 1.0 };

    pub const fn bezier(x1: f64, y1: f64, x2: f64, y2: f64) -> Self {
        EaseCurve::Bezier { x1, y1, x2, y2 }
    }

    pub const fn elastic(dir: EaseDir) -> Self {
        EaseCurve::Elastic { dir, amplitude: 1.0, frequency: 3.0, decay: 6.0 }
    }

    pub const fn bounce(dir: EaseDir) -> Self {
        EaseCurve::Bounce { dir, bounces: 3, restitution: 0.5 }
    }

    /// 種類の名前。
    pub fn kind_label(&self) -> &'static str {
        match self {
            EaseCurve::Bezier { .. } => "ベジェ",
            EaseCurve::Elastic { .. } => "振動",
            EaseCurve::Bounce { .. } => "バウンス",
            EaseCurve::Steps { .. } => "階段",
        }
    }

    /// 値を整える（読み込んだファイルの不正な値で計算が壊れないように）。
    pub fn sanitized(self) -> Self {
        let fin = |v: f64, d: f64| if v.is_finite() { v } else { d };
        match self {
            EaseCurve::Bezier { x1, y1, x2, y2 } => EaseCurve::Bezier {
                x1: fin(x1, 0.0).clamp(0.0, 1.0),
                y1: fin(y1, 0.0).clamp(-4.0, 5.0),
                x2: fin(x2, 1.0).clamp(0.0, 1.0),
                y2: fin(y2, 1.0).clamp(-4.0, 5.0),
            },
            EaseCurve::Elastic { dir, amplitude, frequency, decay } => EaseCurve::Elastic {
                dir,
                amplitude: fin(amplitude, 1.0).clamp(0.0, 10.0),
                frequency: fin(frequency, 3.0).clamp(0.25, 40.0),
                decay: fin(decay, 6.0).clamp(0.0, 60.0),
            },
            EaseCurve::Bounce { dir, bounces, restitution } => {
                EaseCurve::Bounce { dir, bounces: bounces.clamp(1, 20), restitution: fin(restitution, 0.5).clamp(0.05, 0.95) }
            }
            EaseCurve::Steps { count } => EaseCurve::Steps { count: count.clamp(1, 1000) },
        }
    }

    /// 時間の割合 `u`（0..1）での進み具合。`eval(0) = 0`、`eval(1) = 1`。間は 0..1 を外れることがある。
    pub fn eval(&self, u: f64) -> f64 {
        let u = if u.is_finite() { u.clamp(0.0, 1.0) } else { 0.0 };
        if u <= 0.0 {
            return 0.0;
        }
        if u >= 1.0 {
            return 1.0;
        }
        match self.sanitized() {
            EaseCurve::Bezier { x1, y1, x2, y2 } => cubic(x1, y1, x2, y2, u),
            EaseCurve::Elastic { dir, amplitude, frequency, decay } => directed(dir, u, &|v| elastic_out(v, amplitude, frequency, decay)),
            EaseCurve::Bounce { dir, bounces, restitution } => directed(dir, u, &|v| bounce_out(v, bounces, restitution)),
            EaseCurve::Steps { count } => (u * count as f64).floor() / count as f64,
        }
    }

    /// 時間を逆にしたカーブ（キーの時間反転に使う）。`m.eval(u) = 1 - self.eval(1 - u)`。
    pub fn mirrored(&self) -> Self {
        match *self {
            EaseCurve::Bezier { x1, y1, x2, y2 } => EaseCurve::Bezier { x1: 1.0 - x2, y1: 1.0 - y2, x2: 1.0 - x1, y2: 1.0 - y1 },
            EaseCurve::Elastic { dir, amplitude, frequency, decay } => EaseCurve::Elastic { dir: dir.flipped(), amplitude, frequency, decay },
            EaseCurve::Bounce { dir, bounces, restitution } => EaseCurve::Bounce { dir: dir.flipped(), bounces, restitution },
            EaseCurve::Steps { count } => EaseCurve::Steps { count },
        }
    }

    /// CSS の `cubic-bezier(x1, y1, x2, y2)` の書式（ベジェのときだけ）。
    pub fn css(&self) -> Option<String> {
        match self {
            EaseCurve::Bezier { x1, y1, x2, y2 } => Some(format!("cubic-bezier({}, {}, {}, {})", num(*x1), num(*y1), num(*x2), num(*y2))),
            _ => None,
        }
    }

    /// `cubic-bezier(0.25, 0.1, 0.25, 1)` や `0.25,0.1,0.25,1`、`ease-in-out` などの書式を読む。
    pub fn parse_css(text: &str) -> Option<Self> {
        let t = text.trim().trim_end_matches(';').trim();
        let named = match t {
            "linear" => Some(EaseCurve::LINEAR),
            "ease" => Some(EaseCurve::bezier(0.25, 0.1, 0.25, 1.0)),
            "ease-in" => Some(EaseCurve::bezier(0.42, 0.0, 1.0, 1.0)),
            "ease-out" => Some(EaseCurve::bezier(0.0, 0.0, 0.58, 1.0)),
            "ease-in-out" => Some(EaseCurve::bezier(0.42, 0.0, 0.58, 1.0)),
            _ => None,
        };
        if named.is_some() {
            return named;
        }
        let inner = t.strip_prefix("cubic-bezier").map(str::trim).unwrap_or(t);
        let inner = inner.trim_start_matches('(').trim_end_matches(')');
        let v: Vec<f64> = inner.split([',', ' ']).filter(|s| !s.is_empty()).map(|s| s.parse::<f64>().ok()).collect::<Option<_>>()?;
        match v.as_slice() {
            [x1, y1, x2, y2] if (0.0..=1.0).contains(x1) && (0.0..=1.0).contains(x2) => Some(EaseCurve::bezier(*x1, *y1, *x2, *y2).sanitized()),
            _ => None,
        }
    }
}

fn num(v: f64) -> String {
    let s = format!("{:.3}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".into()
    } else {
        s.to_string()
    }
}

/// 終わりで振れる形 `out` から、向きに合わせた形を作る。
fn directed(dir: EaseDir, u: f64, out: &dyn Fn(f64) -> f64) -> f64 {
    match dir {
        EaseDir::Out => out(u),
        EaseDir::In => 1.0 - out(1.0 - u),
        EaseDir::InOut => {
            if u < 0.5 {
                (1.0 - out(1.0 - 2.0 * u)) / 2.0
            } else {
                (1.0 + out(2.0 * u - 1.0)) / 2.0
            }
        }
    }
}

/// 3 次ベジェ (0,0)-(x1,y1)-(x2,y2)-(1,1) の、x = u での y。x1,x2 が 0..1 なら x(s) は単調。
fn cubic(x1: f64, y1: f64, x2: f64, y2: f64, u: f64) -> f64 {
    let bez = |a: f64, b: f64, s: f64| {
        let m = 1.0 - s;
        3.0 * m * m * s * a + 3.0 * m * s * s * b + s * s * s
    };
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if bez(x1, x2, mid) < u {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bez(y1, y2, 0.5 * (lo + hi))
}

/// 終わりで揺れる振動。0 で 0、1 で 1 になるよう、わずかな残りを直線で補う。
fn elastic_out(u: f64, amplitude: f64, frequency: f64, decay: f64) -> f64 {
    let w = std::f64::consts::TAU * frequency;
    let raw = |u: f64| 1.0 - (-decay * u).exp() * ((w * u).cos() + (amplitude - 1.0) * (w * u).sin());
    raw(u) + u * (1.0 - raw(1.0))
}

/// 終わりで跳ねるバウンス。高さ 1 から落ちて、`bounces` 回跳ねて止まる。
fn bounce_out(u: f64, bounces: u32, e: f64) -> f64 {
    // 落下に 1、k 回目の跳ねに 2·e^k の時間がかかる（跳ねる高さは e^(2k)）
    let total = 1.0 + (1..=bounces).map(|k| 2.0 * e.powi(k as i32)).sum::<f64>();
    let mut t = u * total;
    if t < 1.0 {
        return t * t;
    }
    t -= 1.0;
    for k in 1..=bounces {
        let half = e.powi(k as i32);
        if t < 2.0 * half {
            let s = t - half;
            return 1.0 - (half * half - s * s);
        }
        t -= 2.0 * half;
    }
    1.0
}

/// プリセットの分類と名前（イージングの一覧に並ぶ）。
pub struct CurvePreset {
    pub group: &'static str,
    pub name: &'static str,
    pub curve: EaseCurve,
}

const fn p(group: &'static str, name: &'static str, curve: EaseCurve) -> CurvePreset {
    CurvePreset { group, name, curve }
}

/// よく使うイージング。ベジェの値は easings.net の近似と同じ。
pub const CURVE_PRESETS: &[CurvePreset] = &[
    p("基本", "リニア", EaseCurve::LINEAR),
    p("基本", "イーズ", EaseCurve::bezier(0.25, 0.1, 0.25, 1.0)),
    p("基本", "イージーイーズ", EaseCurve::bezier(0.33, 0.0, 0.67, 1.0)),
    p("基本", "強いイーズ", EaseCurve::bezier(0.75, 0.0, 0.25, 1.0)),
    p("サイン", "サイン イン", EaseCurve::bezier(0.12, 0.0, 0.39, 0.0)),
    p("サイン", "サイン アウト", EaseCurve::bezier(0.61, 1.0, 0.88, 1.0)),
    p("サイン", "サイン イン・アウト", EaseCurve::bezier(0.37, 0.0, 0.63, 1.0)),
    p("2 次", "2 次 イン", EaseCurve::bezier(0.11, 0.0, 0.5, 0.0)),
    p("2 次", "2 次 アウト", EaseCurve::bezier(0.5, 1.0, 0.89, 1.0)),
    p("2 次", "2 次 イン・アウト", EaseCurve::bezier(0.45, 0.0, 0.55, 1.0)),
    p("3 次", "3 次 イン", EaseCurve::bezier(0.32, 0.0, 0.67, 0.0)),
    p("3 次", "3 次 アウト", EaseCurve::bezier(0.33, 1.0, 0.68, 1.0)),
    p("3 次", "3 次 イン・アウト", EaseCurve::bezier(0.65, 0.0, 0.35, 1.0)),
    p("4 次", "4 次 イン", EaseCurve::bezier(0.5, 0.0, 0.75, 0.0)),
    p("4 次", "4 次 アウト", EaseCurve::bezier(0.25, 1.0, 0.5, 1.0)),
    p("4 次", "4 次 イン・アウト", EaseCurve::bezier(0.76, 0.0, 0.24, 1.0)),
    p("5 次", "5 次 イン", EaseCurve::bezier(0.64, 0.0, 0.78, 0.0)),
    p("5 次", "5 次 アウト", EaseCurve::bezier(0.22, 1.0, 0.36, 1.0)),
    p("5 次", "5 次 イン・アウト", EaseCurve::bezier(0.83, 0.0, 0.17, 1.0)),
    p("指数", "指数 イン", EaseCurve::bezier(0.7, 0.0, 0.84, 0.0)),
    p("指数", "指数 アウト", EaseCurve::bezier(0.16, 1.0, 0.3, 1.0)),
    p("指数", "指数 イン・アウト", EaseCurve::bezier(0.87, 0.0, 0.13, 1.0)),
    p("円", "円 イン", EaseCurve::bezier(0.55, 0.0, 1.0, 0.45)),
    p("円", "円 アウト", EaseCurve::bezier(0.0, 0.55, 0.45, 1.0)),
    p("円", "円 イン・アウト", EaseCurve::bezier(0.85, 0.0, 0.15, 1.0)),
    p("バック", "バック イン", EaseCurve::bezier(0.36, 0.0, 0.66, -0.56)),
    p("バック", "バック アウト", EaseCurve::bezier(0.34, 1.56, 0.64, 1.0)),
    p("バック", "バック イン・アウト", EaseCurve::bezier(0.68, -0.6, 0.32, 1.6)),
    p("振動", "振動 イン", EaseCurve::elastic(EaseDir::In)),
    p("振動", "振動 アウト", EaseCurve::elastic(EaseDir::Out)),
    p("振動", "振動 イン・アウト", EaseCurve::elastic(EaseDir::InOut)),
    p("バウンス", "バウンス イン", EaseCurve::bounce(EaseDir::In)),
    p("バウンス", "バウンス アウト", EaseCurve::bounce(EaseDir::Out)),
    p("バウンス", "バウンス イン・アウト", EaseCurve::bounce(EaseDir::InOut)),
    p("階段", "階段（4 段）", EaseCurve::Steps { count: 4 }),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_starts_at_0_and_ends_at_1() {
        for pr in CURVE_PRESETS {
            assert_eq!(pr.curve.eval(0.0), 0.0, "{}", pr.name);
            assert_eq!(pr.curve.eval(1.0), 1.0, "{}", pr.name);
            for i in 0..=100 {
                let v = pr.curve.eval(i as f64 / 100.0);
                assert!(v.is_finite() && (-1.5..=2.5).contains(&v), "{} {i}: {v}", pr.name);
            }
            // 時間反転を 2 回すると元に戻る
            let back = pr.curve.mirrored().mirrored();
            for i in 0..=20 {
                let u = i as f64 / 20.0;
                assert!((back.eval(u) - pr.curve.eval(u)).abs() < 1e-9, "{}", pr.name);
            }
        }
    }

    #[test]
    fn mirrored_reverses_time() {
        for pr in CURVE_PRESETS.iter().filter(|p| !matches!(p.curve, EaseCurve::Steps { .. })) {
            let m = pr.curve.mirrored();
            for i in 1..20 {
                let u = i as f64 / 20.0;
                assert!((m.eval(u) - (1.0 - pr.curve.eval(1.0 - u))).abs() < 1e-6, "{} u={u}", pr.name);
            }
        }
    }

    #[test]
    fn bezier_matches_known_values() {
        assert!((EaseCurve::LINEAR.eval(0.3) - 0.3).abs() < 1e-9);
        // イーズアウトは前半で先に進む
        let out = EaseCurve::bezier(0.0, 0.0, 0.58, 1.0);
        assert!(out.eval(0.25) > 0.35);
        // バック アウトは行き過ぎる
        let back = CURVE_PRESETS.iter().find(|p| p.name == "バック アウト").unwrap().curve;
        assert!((0..100).map(|i| back.eval(i as f64 / 100.0)).fold(0.0, f64::max) > 1.05);
    }

    #[test]
    fn elastic_and_bounce_shapes() {
        let el = EaseCurve::elastic(EaseDir::Out);
        assert!((0..100).map(|i| el.eval(i as f64 / 100.0)).fold(0.0, f64::max) > 1.05, "振動は行き過ぎる");
        let b = EaseCurve::bounce(EaseDir::Out);
        let vals: Vec<f64> = (0..=200).map(|i| b.eval(i as f64 / 200.0)).collect();
        assert!(vals.iter().all(|v| *v <= 1.0 + 1e-9), "バウンスは行き過ぎない");
        // 1 の近くで折り返す（跳ねる）
        let touches = vals.windows(3).filter(|w| w[1] >= w[0] && w[1] >= w[2] && w[1] > 0.95 && w[1] < 1.0).count();
        assert!(touches >= 2, "{touches}");
        assert_eq!(EaseCurve::Steps { count: 4 }.eval(0.3), 0.25);
    }

    #[test]
    fn css_round_trip_and_parsing() {
        let c = EaseCurve::bezier(0.25, 0.1, 0.25, 1.0);
        assert_eq!(c.css().unwrap(), "cubic-bezier(0.25, 0.1, 0.25, 1)");
        assert_eq!(EaseCurve::parse_css(&c.css().unwrap()), Some(c));
        assert_eq!(EaseCurve::parse_css("0.42,0,0.58,1"), Some(EaseCurve::bezier(0.42, 0.0, 0.58, 1.0)));
        assert_eq!(EaseCurve::parse_css("ease-in"), Some(EaseCurve::bezier(0.42, 0.0, 1.0, 1.0)));
        assert_eq!(EaseCurve::parse_css("cubic-bezier(1.5, 0, 0, 1)"), None);
        assert_eq!(EaseCurve::parse_css("abc"), None);
    }

    #[test]
    fn serializes_with_type_tag() {
        let j = serde_json::to_string(&EaseCurve::bounce(EaseDir::Out)).unwrap();
        assert!(j.contains("\"type\":\"bounce\""), "{j}");
        assert_eq!(serde_json::from_str::<EaseCurve>(&j).unwrap(), EaseCurve::bounce(EaseDir::Out));
    }
}
