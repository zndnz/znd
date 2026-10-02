//! テキストレイヤーのデータモデルと、アニメーターのセレクター評価。
//! レイアウト（シェーピング・字形）は `znd-text`、描画は `znd-render`。

use crate::property::Property;
use crate::time::Time;
use serde::{Deserialize, Serialize};

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Justify {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextStroke {
    /// sRGB RGBA。
    pub color: [f64; 4],
    pub width: f64,
}

/// ルビ。`start..end` は本文の文字（Unicode スカラー値）インデックス。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ruby {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// 可変フォントの軸（例: wght）。値はアニメート可能。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FontVariation {
    pub tag: String,
    pub value: Property<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextDocument {
    pub text: String,
    /// フォントファミリー名。空なら OS の既定の日本語フォント。
    pub font_family: String,
    pub weight: u16,
    pub italic: bool,
    /// フォントサイズ（px）。
    pub size: f64,
    /// 塗り（sRGB RGBA）。`None` で塗りなし。
    pub fill: Option<[f64; 4]>,
    pub stroke: Option<TextStroke>,
    /// トラッキング（1/1000 em）。
    pub tracking: f64,
    /// 行送り（px）。`None` で自動（サイズの 1.2 倍）。
    pub leading: Option<f64>,
    pub justify: Justify,
    /// 縦書き。
    pub vertical: bool,
    /// 段落テキストのボックス（幅, 高さ）。`None` でポイントテキスト。
    pub box_size: Option<[f64; 2]>,
    pub ruby: Vec<Ruby>,
    /// OpenType 機能。`"palt"` で有効、`"-liga"` で無効。合字（liga）とカーニング（kern）は既定で有効。
    pub features: Vec<String>,
    pub variations: Vec<FontVariation>,
}

impl Default for TextDocument {
    fn default() -> Self {
        TextDocument {
            text: String::new(),
            font_family: String::new(),
            weight: 400,
            italic: false,
            size: 72.0,
            fill: Some([1.0, 1.0, 1.0, 1.0]),
            stroke: None,
            tracking: 0.0,
            leading: None,
            justify: Justify::Left,
            vertical: false,
            box_size: None,
            ruby: Vec::new(),
            features: Vec::new(),
            variations: Vec::new(),
        }
    }
}

impl TextDocument {
    pub fn new(text: impl Into<String>, size: f64) -> Self {
        TextDocument { text: text.into(), size, ..Default::default() }
    }
}

/// パス上テキストの設定。パスはレイヤーのマスク。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextPath {
    /// マスクのインデックス（0 始まり）。
    pub mask: usize,
    #[serde(default)]
    pub reverse: bool,
    /// 文字をパスに垂直に立てる。
    #[serde(default = "yes")]
    pub perpendicular: bool,
    /// 最初のマージン（px）。アニメートするとパス上を流れる。
    pub first_margin: Property<f64>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectorUnit {
    #[default]
    Characters,
    /// 空白を除いた文字。
    CharactersExcludingSpaces,
    Words,
    Lines,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RangeShape {
    /// 矩形。
    #[default]
    Square,
    RampUp,
    RampDown,
    Triangle,
    Round,
    Smooth,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectorMode {
    #[default]
    Add,
    Subtract,
    Intersect,
    Min,
    Max,
    Difference,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TextSelector {
    /// 範囲セレクター。`start`/`end`/`offset` はパーセント。
    Range {
        start: Property<f64>,
        end: Property<f64>,
        offset: Property<f64>,
        /// 量（%、-100..100）。
        amount: Property<f64>,
        #[serde(default)]
        unit: SelectorUnit,
        #[serde(default)]
        shape: RangeShape,
        #[serde(default)]
        mode: SelectorMode,
        /// 順序をランダム化。
        #[serde(default)]
        randomize: bool,
        #[serde(default)]
        seed: u32,
    },
    /// ウィグリーセレクター。文字ごとに min..max（%）の間を揺れる。
    Wiggly {
        max_amount: Property<f64>,
        min_amount: Property<f64>,
        wiggles_per_second: Property<f64>,
        /// 相関（%）。100 で全文字が同じ動き。
        correlation: Property<f64>,
        #[serde(default)]
        unit: SelectorUnit,
        #[serde(default)]
        mode: SelectorMode,
        #[serde(default)]
        seed: u32,
    },
    /// エクスプレッションセレクター。`textIndex`・`textTotal`・`selectorValue` を参照して量（%）を返す。
    Expression {
        source: String,
        #[serde(default)]
        unit: SelectorUnit,
    },
}

impl TextSelector {
    /// 全体を 100% 選択する範囲セレクター。
    pub fn range() -> Self {
        TextSelector::Range {
            start: Property::constant(0.0),
            end: Property::constant(100.0),
            offset: Property::constant(0.0),
            amount: Property::constant(100.0),
            unit: SelectorUnit::Characters,
            shape: RangeShape::Square,
            mode: SelectorMode::Add,
            randomize: false,
            seed: 0,
        }
    }

    pub fn unit(&self) -> SelectorUnit {
        match self {
            TextSelector::Range { unit, .. } | TextSelector::Wiggly { unit, .. } | TextSelector::Expression { unit, .. } => *unit,
        }
    }
}

/// アニメーターが文字に与える変化。`None` の項目は変化させない。値は「選択 100% のときの量」。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AnimatorProps {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Property<[f64; 2]>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<Property<[f64; 2]>>,
    /// パーセント（100 で変化なし）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<Property<[f64; 2]>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotation: Option<Property<f64>>,
    /// 0..100（100 で変化なし）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opacity: Option<Property<f64>>,
    /// 追加トラッキング（1/1000 em）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracking: Option<Property<f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_color: Option<Property<[f64; 4]>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stroke_color: Option<Property<[f64; 4]>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stroke_width: Option<Property<f64>>,
    /// ブラー（px）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blur: Option<Property<f64>>,
    /// 文字オフセット（Unicode コードポイントをずらす）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub character_offset: Option<Property<f64>>,
    /// 文字ごとの 3D: Z 位置と X/Y 回転。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_z: Option<Property<f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotation_x: Option<Property<f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotation_y: Option<Property<f64>>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TextAnimator {
    #[serde(default)]
    pub name: String,
    /// 空なら全文字 100%。
    #[serde(default)]
    pub selectors: Vec<TextSelector>,
    pub props: AnimatorProps,
}

/// セレクター評価のための、1 文字ぶんの位置情報。
#[derive(Debug, Copy, Clone, PartialEq, Default)]
pub struct CharInfo {
    pub is_space: bool,
    /// 空白を除いた通し番号（空白自身は直前の文字の番号）。
    pub index_no_space: usize,
    pub word: usize,
    pub line: usize,
}

/// 文字列から文字ごとの単位情報を作る（改行は文字として数えない）。
pub fn char_infos(text: &str) -> Vec<CharInfo> {
    let mut out = Vec::new();
    let (mut ns, mut word, mut line) = (0usize, 0usize, 0usize);
    let mut in_word = false;
    for ch in text.chars() {
        if ch == '\n' {
            line += 1;
            if in_word {
                word += 1;
                in_word = false;
            }
            continue;
        }
        let space = ch.is_whitespace();
        if space && in_word {
            word += 1;
            in_word = false;
        }
        out.push(CharInfo { is_space: space, index_no_space: if space { ns.saturating_sub(1) } else { ns }, word, line });
        if !space {
            ns += 1;
            in_word = true;
        }
    }
    out
}

fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

fn rand01(a: u32, b: u32) -> f64 {
    hash(a.wrapping_mul(0x9e37_79b1) ^ hash(b)) as f64 / u32::MAX as f64
}

/// 単位インデックスと総数。
fn unit_index(infos: &[CharInfo], i: usize, unit: SelectorUnit) -> (usize, usize) {
    let total = |f: fn(&CharInfo) -> usize| infos.iter().map(f).max().map(|m| m + 1).unwrap_or(0);
    match unit {
        SelectorUnit::Characters => (i, infos.len()),
        SelectorUnit::CharactersExcludingSpaces => (infos[i].index_no_space, total(|c| c.index_no_space)),
        SelectorUnit::Words => (infos[i].word, total(|c| c.word)),
        SelectorUnit::Lines => (infos[i].line, total(|c| c.line)),
    }
}

fn range_value(shape: RangeShape, lo: f64, hi: f64, unit_lo: f64, unit_hi: f64) -> f64 {
    let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
    let center = 0.5 * (unit_lo + unit_hi);
    let span = hi - lo;
    let u = if span > 0.0 {
        ((center - lo) / span).clamp(0.0, 1.0)
    } else if center >= hi {
        1.0
    } else {
        0.0
    };
    let inside = center >= lo && center <= hi && span > 0.0;
    match shape {
        // 矩形は単位区間と選択範囲の重なり率（境界をまたぐ文字が滑らかに切り替わる）
        RangeShape::Square => {
            let w = unit_hi - unit_lo;
            if w <= 0.0 {
                0.0
            } else {
                ((unit_hi.min(hi) - unit_lo.max(lo)) / w).clamp(0.0, 1.0)
            }
        }
        RangeShape::RampUp => u,
        RangeShape::RampDown => 1.0 - u,
        RangeShape::Triangle if inside => 1.0 - (2.0 * u - 1.0).abs(),
        RangeShape::Round if inside => (1.0 - (2.0 * u - 1.0).powi(2)).max(0.0).sqrt(),
        RangeShape::Smooth if inside => 0.5 - 0.5 * (2.0 * std::f64::consts::PI * u).cos(),
        _ => 0.0,
    }
}

/// 時間方向に滑らかな値ノイズ（0..1）。
fn smooth_noise(seed: u32, lane: u32, x: f64) -> f64 {
    let i = x.floor();
    let f = x - i;
    let s = f * f * (3.0 - 2.0 * f);
    let a = rand01(seed ^ lane.wrapping_mul(7919), i as i64 as u32);
    let b = rand01(seed ^ lane.wrapping_mul(7919), (i as i64 + 1) as u32);
    a + (b - a) * s
}

fn combine(mode: SelectorMode, acc: f64, v: f64) -> f64 {
    match mode {
        SelectorMode::Add => acc + v,
        SelectorMode::Subtract => acc - v,
        SelectorMode::Intersect => acc * v,
        SelectorMode::Min => acc.min(v),
        SelectorMode::Max => acc.max(v),
        SelectorMode::Difference => (acc - v).abs(),
    }
}

/// アニメーターの各文字への適用量（-1..1）。
///
/// `expr` はエクスプレッションセレクターの評価関数
/// `(ソース, textIndex(1 始まり), textTotal, selectorValue(%)) -> 量(%)`。
pub fn selector_amounts(
    selectors: &[TextSelector],
    infos: &[CharInfo],
    t: Time,
    expr: &dyn Fn(&str, usize, usize, f64) -> f64,
) -> Vec<f64> {
    if selectors.is_empty() {
        return vec![1.0; infos.len()];
    }
    (0..infos.len())
        .map(|i| {
            let mut acc = 0.0;
            for (si, sel) in selectors.iter().enumerate() {
                let (idx, total) = unit_index(infos, i, sel.unit());
                let total = total.max(1);
                let (mode, v) = match sel {
                    TextSelector::Range { start, end, offset, amount, shape, mode, randomize, seed, .. } => {
                        let off = offset.eval(t) / 100.0;
                        let (lo, hi) = (start.eval(t) / 100.0 + off, end.eval(t) / 100.0 + off);
                        // ランダム順: 単位の並びを決定的にシャッフルする
                        let pos = if *randomize {
                            let mut order: Vec<usize> = (0..total).collect();
                            order.sort_by(|a, b| rand01(*seed, *a as u32).total_cmp(&rand01(*seed, *b as u32)));
                            order.iter().position(|o| *o == idx).unwrap_or(idx)
                        } else {
                            idx
                        };
                        let v = range_value(*shape, lo, hi, pos as f64 / total as f64, (pos + 1) as f64 / total as f64);
                        (*mode, v * amount.eval(t) / 100.0)
                    }
                    TextSelector::Wiggly { max_amount, min_amount, wiggles_per_second, correlation, mode, seed, .. } => {
                        let corr = (correlation.eval(t) / 100.0).clamp(0.0, 1.0);
                        let x = t.seconds() * wiggles_per_second.eval(t);
                        let own = smooth_noise(*seed, idx as u32 + 1, x);
                        let shared = smooth_noise(*seed, 0, x);
                        let n = own + (shared - own) * corr;
                        let (lo, hi) = (min_amount.eval(t) / 100.0, max_amount.eval(t) / 100.0);
                        (*mode, lo + (hi - lo) * n)
                    }
                    TextSelector::Expression { source, .. } => {
                        (SelectorMode::Add, expr(source, idx + 1, total, acc * 100.0) / 100.0 - if si == 0 { 0.0 } else { acc })
                    }
                };
                // 最初のセレクターは「加算」なら値そのもの、「減算」なら反転から始める
                acc = if si == 0 {
                    match mode {
                        SelectorMode::Subtract => 1.0 - v,
                        _ => v,
                    }
                } else {
                    combine(mode, acc, v)
                };
            }
            acc.clamp(-1.0, 1.0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_expr(_: &str, _: usize, _: usize, v: f64) -> f64 {
        v
    }

    fn range(start: f64, end: f64, shape: RangeShape, unit: SelectorUnit) -> TextSelector {
        match TextSelector::range() {
            TextSelector::Range { offset, amount, mode, randomize, seed, .. } => TextSelector::Range {
                start: Property::constant(start),
                end: Property::constant(end),
                offset,
                amount,
                unit,
                shape,
                mode,
                randomize,
                seed,
            },
            _ => unreachable!(),
        }
    }

    #[test]
    fn char_infos_count_words_and_lines() {
        let infos = char_infos("ab cd\nef");
        assert_eq!(infos.len(), 7);
        assert_eq!(infos.iter().map(|c| c.word).collect::<Vec<_>>(), vec![0, 0, 1, 1, 1, 2, 2]);
        assert_eq!(infos.iter().map(|c| c.line).collect::<Vec<_>>(), vec![0, 0, 0, 0, 0, 1, 1]);
        assert_eq!(infos[5].index_no_space, 4);
        assert!(infos[2].is_space);
    }

    #[test]
    fn square_range_selects_with_partial_overlap() {
        let infos = char_infos("abcd");
        let a = selector_amounts(&[range(0.0, 50.0, RangeShape::Square, SelectorUnit::Characters)], &infos, Time::ZERO, &no_expr);
        assert_eq!(a, vec![1.0, 1.0, 0.0, 0.0]);
        let a = selector_amounts(&[range(0.0, 37.5, RangeShape::Square, SelectorUnit::Characters)], &infos, Time::ZERO, &no_expr);
        assert!((a[1] - 0.5).abs() < 1e-9);
    }

    #[test]
    fn shapes_and_units() {
        let infos = char_infos("abcd");
        let up = selector_amounts(&[range(0.0, 100.0, RangeShape::RampUp, SelectorUnit::Characters)], &infos, Time::ZERO, &no_expr);
        assert_eq!(up, vec![0.125, 0.375, 0.625, 0.875]);
        let tri = selector_amounts(&[range(0.0, 100.0, RangeShape::Triangle, SelectorUnit::Characters)], &infos, Time::ZERO, &no_expr);
        assert!((tri[0] - 0.25).abs() < 1e-9 && (tri[1] - 0.75).abs() < 1e-9 && (tri[1] - tri[2]).abs() < 1e-9);
        // 単語単位: 「ab cd」の前半 50% = 最初の単語（とそれに続く空白は次の単語側）
        let infos = char_infos("ab cd");
        let w = selector_amounts(&[range(0.0, 50.0, RangeShape::Square, SelectorUnit::Words)], &infos, Time::ZERO, &no_expr);
        assert_eq!(w, vec![1.0, 1.0, 0.0, 0.0, 0.0]);
        assert_eq!(selector_amounts(&[], &infos, Time::ZERO, &no_expr), vec![1.0; 5]);
    }

    #[test]
    fn wiggly_is_deterministic_bounded_and_correlated() {
        let infos = char_infos("abcdefgh");
        let wig = |corr: f64| TextSelector::Wiggly {
            max_amount: Property::constant(100.0),
            min_amount: Property::constant(-100.0),
            wiggles_per_second: Property::constant(3.0),
            correlation: Property::constant(corr),
            unit: SelectorUnit::Characters,
            mode: SelectorMode::Add,
            seed: 5,
        };
        let t = Time::from_seconds(0.37);
        let a = selector_amounts(&[wig(0.0)], &infos, t, &no_expr);
        assert_eq!(a, selector_amounts(&[wig(0.0)], &infos, t, &no_expr));
        assert!(a.iter().all(|v| (-1.0..=1.0).contains(v)));
        assert!(a.iter().any(|v| (v - a[0]).abs() > 1e-3));
        let c = selector_amounts(&[wig(100.0)], &infos, t, &no_expr);
        assert!(c.iter().all(|v| (v - c[0]).abs() < 1e-12));
    }

    #[test]
    fn expression_selector_receives_index_and_total() {
        let infos = char_infos("abc");
        let sel = TextSelector::Expression { source: "x".into(), unit: SelectorUnit::Characters };
        let a = selector_amounts(&[sel], &infos, Time::ZERO, &|_, index, total, _| 100.0 * index as f64 / total as f64);
        assert!((a[0] - 1.0 / 3.0).abs() < 1e-9 && (a[2] - 1.0).abs() < 1e-9);
    }
}
