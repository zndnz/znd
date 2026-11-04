//! 音ハメの自動配置: ビート（ビートグリッド・マーカー）の時刻に合わせて、レイヤーを並べたり、
//! プロパティにパルス（拍ごとの一瞬の変化）のキーフレームを打ったりする。
//!
//! - [`stamp_on_beats`] … レイヤーをビートごとに並べる。頭出し（毎回素材の同じ位置から＝連打）と、
//!   続けて再生（素材は流れたまま、ビートで切れ目を入れる）の 2 通り。
//! - [`pulse_on_beats`] … 選んだプロパティに、ビートで `delta` だけ跳ねて `length` で戻るキーを打つ。
//! - [`sync_test_comp`] … 映像と音のずれを確かめるためのコンポ（拍ごとに白く光る）。

use crate::beat::{nearest_frame, BeatGrid};
use crate::edit;
use crate::model::{Composition, Layer, LayerId, LayerKind, Marker, Transform};
use crate::property::{Animatable, Ease, Interp, Keyframe, Property};
use crate::time::{FrameRate, Time};
use crate::visit::{self, PropMut};

/// ビートの取り方。
#[derive(Debug, Clone, PartialEq)]
pub enum BeatSource {
    /// ビートグリッドの `every` 拍ごと（1 = 毎拍、2 = 2 拍ごと、小節頭だけなら拍子の数）。
    /// `every` が 0 ならグリッドの分割（16 分など）ごと。
    Grid { every: u32 },
    /// コンポのマーカー（ビート検出・タップ入力で打ったもの）。
    CompMarkers,
    /// レイヤーのマーカー。
    LayerMarkers(LayerId),
}

/// `range` 内（開始を含み、終了を含まない）のビートの時刻。フレーム境界に丸め、重複は除く。
pub fn beat_times(comp: &Composition, source: &BeatSource, range: (Time, Time)) -> Vec<Time> {
    let fr = comp.frame_rate;
    let (a, b) = range;
    let mut out: Vec<Time> = match source {
        BeatSource::Grid { every } => match &comp.beat_grid {
            Some(g) if g.bpm > 0.0 => {
                let mut grid = *g;
                if *every > 0 {
                    grid.subdivision = 1;
                }
                let step = (*every).max(1) as i64;
                grid.lines(a, b)
                    .into_iter()
                    .filter(|(t, _, _)| *every == 0 || (g.beat_at(*t).round() as i64).rem_euclid(step) == 0)
                    .map(|(t, _, _)| t)
                    .collect()
            }
            _ => Vec::new(),
        },
        BeatSource::CompMarkers => comp.markers.iter().map(|m| m.time).collect(),
        BeatSource::LayerMarkers(id) => comp.layer(id).map(|l| l.markers.iter().map(|m| m.time).collect()).unwrap_or_default(),
    };
    for t in &mut out {
        *t = nearest_frame(*t, fr);
    }
    out.retain(|t| *t >= a && *t < b);
    out.sort();
    out.dedup();
    out
}

/// ビートでの並べ方。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StampMode {
    /// 毎回、素材の同じ位置（元のレイヤーのイン点の位置）から再生する。音MAD の連打・頭出し。
    Restart,
    /// 素材は流れたまま、ビートの位置で切れ目を入れる（カットの位置だけをビートに合わせる）。
    Continue,
}

/// レイヤーを `times` の各ビートから次のビート（最後は `end`）まで並べる。
/// 元のレイヤーが最初の区間になり、残りは複製して元のすぐ上に置く（新しいものほど上）。
/// キーフレームとマーカーはレイヤーと一緒に動く。作った（元を含む）レイヤーの ID を時刻順に返す。
pub fn stamp_on_beats(
    comp: &mut Composition,
    id: &LayerId,
    times: &[Time],
    end: Time,
    mode: StampMode,
    fresh_id: &mut dyn FnMut() -> LayerId,
) -> Vec<LayerId> {
    let fr = comp.frame_rate;
    let Some(index) = comp.layer_index(id) else { return Vec::new() };
    let times: Vec<Time> = times.iter().copied().filter(|t| *t < end).collect();
    if times.is_empty() {
        return Vec::new();
    }
    let original = comp.layers[index].clone();
    let min_len = fr.frame_to_time(1);
    let mut out = Vec::new();
    for (i, &t) in times.iter().enumerate() {
        let seg_end = times.get(i + 1).copied().unwrap_or(end).max(t + min_len);
        let mut l = original.clone();
        let dt = match mode {
            StampMode::Restart => t - original.in_point,
            StampMode::Continue => times[0] - original.in_point,
        };
        edit::shift_layer(&mut l, dt);
        l.in_point = t;
        l.out_point = seg_end;
        l.markers.retain(|m| m.time >= t && m.time < seg_end);
        if i == 0 {
            comp.layers[index] = l;
            out.push(original.id.clone());
        } else {
            l.id = fresh_id();
            l.name = format!("{} {}", original.name, i + 1);
            out.push(l.id.clone());
            comp.layers.insert(index, l);
        }
    }
    out
}

fn pulse<V: Animatable>(p: &mut Property<V>, times: &[Time], delta: &[f64], length: Time) -> usize {
    // 打つ前の値を基準にする（既存のアニメーションの上に重ねる）
    let base: Vec<(Time, V, V)> = times
        .iter()
        .map(|t| {
            let rest_at_peak = p.eval(*t);
            let rest_after = p.eval(*t + length);
            let comps: Vec<f64> =
                (0..rest_at_peak.dims()).map(|d| rest_at_peak.component(d) + delta.get(d).or(delta.last()).copied().unwrap_or(0.0)).collect();
            (*t, rest_at_peak.with_components(&comps), rest_after)
        })
        .collect();
    let dims = if p.spatial { 1 } else { base.first().map(|b| b.1.dims()).unwrap_or(1) };
    let mut n = 0;
    for (t, peak, rest) in base {
        // ピークは一瞬で立ち上がり（入る側はリニア）、出る側はゆっくり戻る
        let k = Keyframe::linear(t, peak).with_ease(Vec::new(), vec![Ease { speed: 0.0, influence: 0.6 }; dims]);
        let k = Keyframe { interp_out: Interp::Bezier, ..k };
        let r = Keyframe::easy_ease(t + length, rest);
        for k in [k, r] {
            match p.keyframes.binary_search_by_key(&k.time, |e| e.time) {
                Ok(i) => p.keyframes[i] = k,
                Err(i) => p.keyframes.insert(i, k),
            }
            n += 1;
        }
    }
    n
}

/// プロパティ `path` に、各ビートで `delta` だけ跳ねて `length` 後に元へ戻るキーを打つ。
/// `delta` は成分ごと（足りなければ最後の値を使う）。打ったキーの数を返す（パスのプロパティは対象外で 0）。
pub fn pulse_on_beats(layer: &mut Layer, path: &str, times: &[Time], delta: &[f64], length: Time) -> usize {
    if length <= Time::ZERO {
        return 0;
    }
    let mut n = 0;
    visit::visit_layer_mut(layer, &mut |p, m| {
        if p != path {
            return;
        }
        n += match m {
            PropMut::F1(p) => pulse(p, times, delta, length),
            PropMut::F2(p) => pulse(p, times, delta, length),
            PropMut::F3(p) => pulse(p, times, delta, length),
            PropMut::F4(p) => pulse(p, times, delta, length),
            PropMut::Path(_) => 0,
        };
    });
    n
}

/// 映像と音のずれを確かめるためのコンポ。拍ごとに画面全体が 2 フレームだけ白く光る（小節頭は赤）。
/// 同じ拍にクリック音を置けば（`znd_audio::click_track`）、光と音が同時に感じられるかで確かめられる。
pub fn sync_test_comp(id: &str, width: u32, height: u32, fr: FrameRate, bpm: f64, seconds: f64) -> Composition {
    let duration = Time::from_seconds(seconds);
    let mut comp = Composition::new(id, "同期テスト", width, height, fr, duration);
    let grid = BeatGrid::new(bpm);
    comp.beat_grid = Some(grid);
    let (w, h) = (width as f64, height as f64);
    let flash_len = fr.frame_to_time(2);
    for (accent, color, name) in [(false, [1.0, 1.0, 1.0], "拍"), (true, [1.0, 0.15, 0.1], "小節頭")] {
        let mut l = Layer::new(
            format!("{id}-{}", if accent { "bar" } else { "beat" }),
            name,
            LayerKind::Solid { width, height, color },
            Transform::centered(w, h, w, h),
            duration,
        );
        let mut keys = vec![Keyframe::hold(Time::ZERO, 0.0)];
        for (t, bar, _) in grid.lines(Time::ZERO, duration).into_iter().filter(|(t, _, _)| *t < duration) {
            if bar == accent || (!accent && bar) {
                let t = nearest_frame(t, fr);
                keys.push(Keyframe::hold(t, 100.0));
                keys.push(Keyframe::hold(t + flash_len, 0.0));
            }
        }
        let mut p = Property::constant(0.0).with_keys(keys);
        p.normalize_keys();
        l.transform.opacity = p;
        comp.layers.push(l);
    }
    // 小節頭を上に
    comp.layers.reverse();
    comp.markers = grid
        .lines(Time::ZERO, duration)
        .into_iter()
        .filter(|(t, bar, _)| *bar && *t < duration)
        .map(|(t, _, _)| Marker { time: nearest_frame(t, fr), duration: Time::ZERO, comment: String::new() })
        .collect();
    comp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comp() -> Composition {
        let mut c = Composition::new("c", "c", 640, 360, FrameRate::FPS_30, Time::from_seconds(8.0));
        c.beat_grid = Some(BeatGrid::new(120.0));
        let mut l = Layer::new("v", "素材", LayerKind::Null, Transform::centered(1.0, 1.0, 640.0, 360.0), c.duration);
        l.in_point = Time::from_seconds(1.0);
        l.start_time = Time::from_seconds(0.5);
        l.transform.opacity = Property::constant(100.0).with_keys(vec![Keyframe::linear(Time::from_seconds(1.0), 0.0)]);
        c.layers.push(l);
        c
    }

    fn f(fr: FrameRate, t: Time) -> i64 {
        fr.time_to_frame(t)
    }

    #[test]
    fn grid_and_marker_sources() {
        let mut c = comp();
        let fr = c.frame_rate;
        let all = beat_times(&c, &BeatSource::Grid { every: 1 }, (Time::ZERO, Time::from_seconds(2.0)));
        assert_eq!(all.iter().map(|t| f(fr, *t)).collect::<Vec<_>>(), vec![0, 15, 30, 45]);
        let bars = beat_times(&c, &BeatSource::Grid { every: 4 }, (Time::ZERO, Time::from_seconds(5.0)));
        assert_eq!(bars.iter().map(|t| f(fr, *t)).collect::<Vec<_>>(), vec![0, 60, 120]);
        c.markers = vec![Marker { time: Time::from_seconds(0.51), duration: Time::ZERO, comment: String::new() }];
        let m = beat_times(&c, &BeatSource::CompMarkers, (Time::ZERO, c.duration));
        assert_eq!(f(fr, m[0]), 15);
        c.beat_grid = None;
        assert!(beat_times(&c, &BeatSource::Grid { every: 1 }, (Time::ZERO, c.duration)).is_empty());
    }

    #[test]
    fn restart_mode_plays_same_source_point_on_every_beat() {
        let mut c = comp();
        let fr = c.frame_rate;
        let times = beat_times(&c, &BeatSource::Grid { every: 1 }, (Time::from_seconds(2.0), Time::from_seconds(4.0)));
        let mut n = 0;
        let ids = stamp_on_beats(&mut c, &LayerId::new("v"), &times, Time::from_seconds(4.0), StampMode::Restart, &mut || {
            n += 1;
            LayerId::new(format!("v{n}"))
        });
        assert_eq!(ids.len(), 4);
        assert_eq!(c.layers.len(), 4);
        for (i, id) in ids.iter().enumerate() {
            let l = c.layer(id).unwrap();
            assert_eq!(f(fr, l.in_point), 60 + i as i64 * 15);
            assert_eq!(f(fr, l.out_point), 75 + i as i64 * 15);
            // 素材の再生位置（イン点 - 開始時刻）はどれも元と同じ 0.5 秒
            assert_eq!(l.in_point - l.start_time, Time::from_seconds(0.5));
            // キーもイン点と一緒に動く
            assert_eq!(l.transform.opacity.keyframes[0].time, l.in_point);
        }
        // 後のものほど上
        assert_eq!(c.layers[0].id, ids[3]);
    }

    #[test]
    fn continue_mode_keeps_source_flowing() {
        let mut c = comp();
        let fr = c.frame_rate;
        let times = vec![Time::from_seconds(2.0), Time::from_seconds(2.5)];
        let ids = stamp_on_beats(&mut c, &LayerId::new("v"), &times, Time::from_seconds(3.0), StampMode::Continue, &mut || {
            LayerId::new("x")
        });
        let (a, b) = (c.layer(&ids[0]).unwrap(), c.layer(&ids[1]).unwrap());
        assert_eq!(a.start_time, b.start_time);
        assert_eq!((f(fr, a.out_point), f(fr, b.in_point)), (75, 75));
    }

    #[test]
    fn pulse_adds_peak_and_return_keys() {
        let mut c = comp();
        let l = &mut c.layers[0];
        l.transform.scale = Property::constant([100.0, 100.0]);
        let times = [Time::from_seconds(1.0), Time::from_seconds(2.0)];
        let n = pulse_on_beats(l, "transform.scale", &times, &[20.0], FrameRate::FPS_30.frame_to_time(6));
        assert_eq!(n, 4);
        assert_eq!(l.transform.scale.eval(times[0]), [120.0, 120.0]);
        let back = l.transform.scale.eval(times[0] + FrameRate::FPS_30.frame_to_time(6));
        assert!((back[0] - 100.0).abs() < 1e-9 && (back[1] - 100.0).abs() < 1e-9, "{back:?}");
        assert_eq!(pulse_on_beats(l, "nope", &times, &[1.0], Time::from_seconds(0.1)), 0);
    }

    #[test]
    fn sync_test_flashes_on_every_beat() {
        let c = sync_test_comp("s", 320, 180, FrameRate::FPS_30, 120.0, 4.0);
        let fr = c.frame_rate;
        let beat = c.layers.iter().find(|l| l.name == "拍").unwrap();
        let bar = c.layers.iter().find(|l| l.name == "小節頭").unwrap();
        for frame in [15, 16, 45] {
            assert_eq!(beat.transform.opacity.eval(fr.frame_to_time(frame)), 100.0, "frame {frame}");
        }
        assert_eq!(beat.transform.opacity.eval(fr.frame_to_time(17)), 0.0);
        assert_eq!(bar.transform.opacity.eval(fr.frame_to_time(60)), 100.0);
        assert_eq!(bar.transform.opacity.eval(fr.frame_to_time(15)), 0.0);
        assert_eq!(c.markers.len(), 2);
    }
}
