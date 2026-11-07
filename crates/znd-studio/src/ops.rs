//! レイヤー・キーフレーム・マーカー・音声からの生成など、znd-core の編集の関数をそのまま使う操作。
//!
//! 画面は `op` に操作の名前と引数を送り、Rust がプロジェクトを書き換えて取り消し履歴に積む。
//! 結果には、操作のあとに選ぶレイヤーやキーフレームも入る（複製したレイヤーを選ぶ、など）。

use crate::{lock, Shared, Snapshot, Studio};
use serde::{Deserialize, Serialize};
use tauri::State;
use znd_audio::analysis::{band_keyframes, detect_onsets, Band};
use znd_core::beatfit::{self, BeatSource, StampMode};
use znd_core::keys::{self, KeyAddr, KeyClipboard};
use znd_core::*;

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Duplicate { layer: String },
    Delete { layer: String },
    /// 再生ヘッドで 2 つに分ける。
    Split { layer: String },
    TrimIn { layer: String },
    TrimOut { layer: String },
    /// イン点（アウト点）が再生ヘッドに来るよう、レイヤーを動かす。
    MoveIn { layer: String },
    MoveOut { layer: String },
    /// "front" / "forward" / "backward" / "back"（行ごと動かす）。
    Reorder { layer: String, to: String },
    /// 1 行に並んでいるレイヤーから離して、自分だけの行にする。
    NewRow { layer: String },
    /// `target` のいる行に移す（時間の重なりは切り詰める）。
    JoinRow { layer: String, target: String },
    /// コマ打ち（1 で解除）。
    Hold { layer: String, frames: u32 },
    Precompose { layer: String },
    /// テキストをシェイプに変換する。
    Outline { layer: String },
    /// 親を付ける（循環するなら付けない）。
    Parent { layer: String, parent: Option<String> },
    /// 揃える。`mode` は -1（左・上）/ 0（中央）/ 1（右・下）。
    Align { layer: String, horizontal: bool, mode: i32 },
    Marker { comment: String },
    ClearMarkers,
    /// 音の立ち上がりをマーカーにする（`layer` があればそのレイヤーの音、無ければコンポ全体）。
    DetectBeats { layer: Option<String>, sensitivity: f64 },
    /// 帯域の音量を、選んだレイヤーのスケール（100〜150%）のキーフレームにする。
    BandScale { layer: String, source: Option<String>, band: String },
    /// 空コメントのマーカー（タップ）から BPM を推定してビートグリッドにする。
    BpmFromTaps,
    /// コンポのロック（自分の名前で）・解除。
    LockComp { on: bool },
    /// 選んだキーの時間を反転する。
    ReverseKeys { layer: String, keys: Vec<KeyAddr> },
    /// 選んだキーの時間を `percent` % に伸縮する（先頭のキーを中心に）。
    ScaleKeys { layer: String, keys: Vec<KeyAddr>, percent: f64 },
    /// キーをクリップボードの文字にする（プロジェクトは変えない）。
    CopyKeys { layer: String, keys: Vec<KeyAddr> },
    /// クリップボードの文字のキーを、再生ヘッドの位置に貼る。`target` は最後に触ったプロパティ。
    PasteKeys { layer: String, text: String, target: Option<String> },
    /// ビートの位置にレイヤーを並べる（`restart` なら毎回素材の頭から）。
    Stamp { layer: String, source: usize, range: Option<(i64, i64)>, restart: bool },
    /// ビートの位置で、プロパティを跳ねさせて戻すキーを打つ。
    Pulse { layer: String, source: usize, range: Option<(i64, i64)>, path: String, delta: f64, frames: u32 },
    /// 120 BPM のクリック音と光る画面の、同期テスト用のコンポを作る。
    SyncTest,
    /// レイヤーの中央に矩形（`ellipse` なら楕円）のマスクを足す。
    AddMask { layer: String, ellipse: bool },
}

#[derive(Deserialize)]
pub struct OpCtx {
    comp: String,
    frame: i64,
}

#[derive(Serialize, Default)]
pub struct OpResult {
    snapshot: Option<Snapshot>,
    status: String,
    /// 操作のあとに選ぶレイヤー（`Some(None)` で選択を外す。`None` なら変えないので、送らない）。
    #[serde(skip_serializing_if = "Option::is_none")]
    select: Option<Option<String>>,
    /// 操作のあとに選ぶキーフレーム。
    keys: Option<Vec<KeyAddr>>,
    /// コピーしたキーフレーム（クリップボードに入れる文字）。
    clip: Option<String>,
    /// 切り替えるコンポ。
    comp: Option<String>,
}

/// ビートの取り方（画面の選択欄の順）。
pub const BEAT_SOURCES: [&str; 6] = ["毎拍（ビートグリッド）", "2 拍ごと", "小節頭", "グリッドの分割ごと", "コンポのマーカー", "選択レイヤーのマーカー"];

fn beat_source(comp: &Composition, i: usize, layer: &LayerId) -> BeatSource {
    let bar = comp.beat_grid.map(|g| g.beats_per_bar.max(1)).unwrap_or(4);
    match i {
        0 => BeatSource::Grid { every: 1 },
        1 => BeatSource::Grid { every: 2 },
        2 => BeatSource::Grid { every: bar },
        3 => BeatSource::Grid { every: 0 },
        4 => BeatSource::CompMarkers,
        _ => BeatSource::LayerMarkers(layer.clone()),
    }
}

fn beat_plan(comp: &Composition, source: usize, range: Option<(i64, i64)>, layer: &LayerId) -> ((Time, Time), Vec<Time>) {
    let fr = comp.frame_rate;
    let range = range.map(|(a, b)| (fr.frame_to_time(a), fr.frame_to_time(b))).unwrap_or((Time::ZERO, comp.duration));
    (range, beatfit::beat_times(comp, &beat_source(comp, source, layer), range))
}

/// 音（モノラル）。`layer` に音があればそのレイヤー、無ければコンポ全体のミックス。
fn mono(s: &Studio, comp: &Composition, layer: Option<&LayerId>) -> Result<Vec<f32>, String> {
    let engine = s.engine.as_ref().ok_or("プレビューを準備中です")?;
    let mixer = znd_audio::Mixer::new(&s.project, &*engine.clips);
    let frames = mixer.comp_frames(comp);
    let has_audio = |id: &LayerId| comp.layer(id).is_some_and(|l| znd_audio::layer_footage(&s.project, l).is_some());
    let stereo = match layer.filter(|id| has_audio(id)) {
        Some(id) => mixer.mix_layer(&comp.id, id, 0, frames)?,
        None => mixer.mix(&comp.id, 0, frames)?,
    };
    Ok(stereo.chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5).collect())
}

impl Studio {
    /// 現在のコンポを書き換える。ほかの人がロックしているコンポは書き換えない。
    /// `f` は（コンポ, プロジェクト全体の読み取り用の複製）を受け取り、結果の文を返す。Err なら何も変えない。
    fn edit_comp(&mut self, comp: &CompId, label: &str, f: impl FnOnce(&mut Composition) -> Result<String, String>) -> OpResult {
        if let Some(owner) = self.locked_by_other(comp) {
            return OpResult { status: format!("このコンポは {owner} がロックしているため編集できません"), ..Default::default() };
        }
        let id = comp.clone();
        let r = self.mutate(label, |p| {
            let c = p.comp_mut(&id).ok_or("コンポがありません")?;
            let status = f(c)?;
            edit::normalize_tracks(c);
            Ok(status)
        });
        OpResult { snapshot: r.snapshot, status: r.status, ..Default::default() }
    }

    pub(crate) fn locked_by_other(&self, comp: &CompId) -> Option<String> {
        self.project.comp(comp).and_then(|c| c.locked_by.clone()).filter(|o| o != &self.user)
    }
}

fn layer_op(s: &mut Studio, ctx: &OpCtx, op: Op) -> OpResult {
    let comp_id = CompId::new(ctx.comp.as_str());
    let Some(comp) = s.project.comp(&comp_id).cloned() else { return OpResult { status: "コンポがありません".into(), ..Default::default() } };
    let fr = comp.frame_rate;
    let now = fr.frame_to_time(ctx.frame.clamp(0, (comp.frame_count() - 1).max(0)));
    let layer_of = |id: &str| comp.layer(&LayerId::new(id)).cloned().ok_or_else(|| "レイヤーがありません".to_string());
    let unlocked = |id: &str| -> Result<Layer, String> {
        let l = layer_of(id)?;
        if l.locked {
            return Err(format!("「{}」はロックされています", l.name));
        }
        Ok(l)
    };
    macro_rules! fail {
        ($e:expr) => {
            return OpResult { status: $e, ..Default::default() }
        };
    }
    match op {
        Op::Duplicate { layer } => {
            let new_id = LayerId::new(s.project.fresh_id("layer"));
            let created = new_id.clone();
            let mut r = s.edit_comp(&comp_id, "レイヤーを複製", |c| {
                edit::duplicate_layer(c, &LayerId::new(layer), new_id).then(String::new).ok_or_else(|| "レイヤーがありません".into())
            });
            if r.snapshot.is_some() {
                r.select = Some(Some(created.0));
                r.keys = Some(Vec::new());
            }
            r
        }
        Op::Delete { layer } => {
            if let Err(e) = unlocked(&layer) {
                fail!(format!("{e}（削除できません）"));
            }
            let mut r = s.edit_comp(&comp_id, "レイヤーを削除", |c| {
                edit::remove_layer(c, &LayerId::new(layer)).map(|_| String::new()).ok_or_else(|| "レイヤーがありません".into())
            });
            if r.snapshot.is_some() {
                r.select = Some(None);
                r.keys = Some(Vec::new());
            }
            r
        }
        Op::Split { layer } => {
            let l = match unlocked(&layer) {
                Ok(l) => l,
                Err(e) => fail!(e),
            };
            let new_id = LayerId::new(s.project.fresh_id("layer"));
            let created = new_id.clone();
            let mut r = s.edit_comp(&comp_id, "レイヤーを分割", |c| {
                if edit::split_layer(c, &l.id, now, new_id) {
                    Ok(format!("「{}」を再生ヘッドで分割しました", l.name))
                } else {
                    Err("再生ヘッドがレイヤーのイン点とアウト点の間にありません".into())
                }
            });
            if r.snapshot.is_some() {
                r.select = Some(Some(created.0));
                r.keys = Some(Vec::new());
            }
            r
        }
        Op::TrimIn { layer } | Op::TrimOut { layer } | Op::MoveIn { layer } | Op::MoveOut { layer } if unlocked(&layer).is_err() => {
            fail!(unlocked(&layer).err().unwrap_or_default())
        }
        Op::TrimIn { layer } => s.edit_comp(&comp_id, "イン点を再生ヘッドに", |c| {
            edit::trim_in(c.layer_mut(&LayerId::new(layer)).ok_or("レイヤーがありません")?, now, fr);
            Ok(String::new())
        }),
        Op::TrimOut { layer } => s.edit_comp(&comp_id, "アウト点を再生ヘッドに", |c| {
            edit::trim_out(c.layer_mut(&LayerId::new(layer)).ok_or("レイヤーがありません")?, now + fr.frame_to_time(1), fr);
            Ok(String::new())
        }),
        Op::MoveIn { layer } => s.edit_comp(&comp_id, "イン点を再生ヘッドへ移動", |c| {
            let l = c.layer_mut(&LayerId::new(layer)).ok_or("レイヤーがありません")?;
            let dt = now - l.in_point;
            edit::shift_layer(l, dt);
            Ok(String::new())
        }),
        Op::MoveOut { layer } => s.edit_comp(&comp_id, "アウト点を再生ヘッドへ移動", |c| {
            let l = c.layer_mut(&LayerId::new(layer)).ok_or("レイヤーがありません")?;
            let dt = now + fr.frame_to_time(1) - l.out_point;
            edit::shift_layer(l, dt);
            Ok(String::new())
        }),
        Op::Reorder { layer, to } => {
            let id = LayerId::new(layer);
            let rows = edit::track_rows(&comp);
            let Some(k) = comp.layer_index(&id).and_then(|i| rows.iter().position(|r| r.contains(&i))) else { fail!("レイヤーがありません".into()) };
            let last = rows.len() - 1;
            let dest = match to.as_str() {
                "front" if k > 0 => 0,
                "forward" if k > 0 => rows[k - 1].start,
                "backward" if k < last => rows[k + 1].end,
                "back" if k < last => comp.layers.len(),
                _ => fail!(String::new()),
            };
            s.edit_comp(&comp_id, "重ね順", |c| edit::reorder_row(c, &id, dest).then(String::new).ok_or_else(String::new))
        }
        Op::NewRow { layer } => {
            let id = LayerId::new(layer);
            let Some(i) = comp.layer_index(&id) else { fail!("レイヤーがありません".into()) };
            let start = edit::row_range(&comp, i).start;
            s.edit_comp(&comp_id, "行から出す", |c| edit::move_to_new_row(c, &id, start).then(String::new).ok_or_else(String::new))
        }
        Op::JoinRow { layer, target } => {
            let mut n = 0;
            let base = s.project.fresh_id("layer");
            s.edit_comp(&comp_id, "同じ行に並べる", |c| {
                let id = LayerId::new(layer);
                if !edit::move_to_row(c, &id, &LayerId::new(target)) {
                    return Err(String::new());
                }
                edit::trim_row_overlaps(c, &id, &mut || {
                    n += 1;
                    LayerId::new(format!("{base}-{n}"))
                });
                Ok(String::new())
            })
        }
        Op::Hold { layer, frames } => s.edit_comp(&comp_id, "コマ打ち", |c| {
            c.layer_mut(&LayerId::new(layer)).ok_or("レイヤーがありません")?.hold_frames = frames.max(1);
            Ok(if frames <= 1 { "コマ打ちを解除しました".into() } else { format!("{frames} コマ打ちにしました") })
        }),
        Op::Precompose { layer } => {
            let Ok(l) = layer_of(&layer) else { fail!("レイヤーがありません".into()) };
            if let Some(owner) = s.locked_by_other(&comp_id) {
                fail!(format!("このコンポは {owner} がロックしているため編集できません"));
            }
            let new_comp = CompId::new(s.project.fresh_id("comp"));
            let mut nested = Composition::new(new_comp.0.clone(), format!("{} コンポ", l.name), comp.width, comp.height, comp.frame_rate, comp.duration);
            // 親とマットは元のコンポに残る（中へ持っていくと参照が切れる）
            let mut inner = l.clone();
            inner.parent = None;
            inner.track_matte = None;
            nested.layers.push(inner);
            let mut outer = Layer::new(
                l.id.0.clone(),
                l.name.clone(),
                LayerKind::Precomp { comp: new_comp },
                Transform::centered(comp.width as f64, comp.height as f64, comp.width as f64, comp.height as f64),
                comp.duration,
            );
            outer.parent = l.parent.clone();
            outer.track_matte = l.track_matte.clone();
            outer.in_point = l.in_point;
            outer.out_point = l.out_point;
            outer.track = l.track;
            let r = s.mutate("プリコンポーズ", |p| {
                *p.comp_mut(&comp_id).ok_or("コンポがありません")?.layer_mut(&l.id).ok_or("レイヤーがありません")? = outer;
                p.compositions.push(nested);
                Ok(format!("「{}」をプリコンポーズしました", l.name))
            });
            OpResult { snapshot: r.snapshot, status: r.status, ..Default::default() }
        }
        Op::Outline { layer } => s.edit_comp(&comp_id, "テキストをシェイプに変換", |c| {
            let l = c.layer_mut(&LayerId::new(layer)).ok_or("レイヤーがありません")?;
            let LayerKind::Text { document, .. } = &l.kind else { return Err("テキストのレイヤーではありません".into()) };
            let fill = document.fill.unwrap_or([1.0, 1.0, 1.0, 1.0]);
            let mut items: Vec<ShapeItem> = znd_text::outlines(document, now).into_iter().map(ShapeItem::path).collect();
            items.push(ShapeItem::fill(fill));
            l.kind = LayerKind::Shape { items };
            Ok("シェイプに変換しました".into())
        }),
        Op::Parent { layer, parent } => {
            let r = s.edit_comp(&comp_id, "親", |c| {
                c.layer_mut(&LayerId::new(layer)).ok_or("レイヤーがありません")?.parent = parent.map(LayerId::new);
                Ok(String::new())
            });
            if r.snapshot.is_none() && r.status.contains("できません") {
                return OpResult { status: "親子関係が循環するため設定できません".into(), ..Default::default() };
            }
            r
        }
        Op::Align { layer, horizontal, mode } => {
            let Ok(l) = layer_of(&layer) else { fail!("レイヤーがありません".into()) };
            let m = eval::world_matrix(&comp, &l, now);
            // 寸法の無いレイヤー（シェイプ・テキストなど）はアンカーポイントの位置で揃える
            let points: Vec<(f64, f64)> = match s.project.source_size(&l.kind) {
                Some((w, h)) => [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)].iter().map(|(x, y)| m.apply(*x, *y)).collect(),
                None => {
                    let a = l.transform.anchor.eval(now);
                    vec![m.apply(a[0], a[1])]
                }
            };
            let axis = |p: &(f64, f64)| if horizontal { p.0 } else { p.1 };
            let lo = points.iter().map(axis).fold(f64::INFINITY, f64::min);
            let hi = points.iter().map(axis).fold(f64::NEG_INFINITY, f64::max);
            let size = if horizontal { comp.width } else { comp.height } as f64;
            let d = match mode {
                -1 => -lo,
                0 => size / 2.0 - (lo + hi) / 2.0,
                _ => size - hi,
            };
            let delta = if horizontal { [d, 0.0] } else { [0.0, d] };
            s.edit_comp(&comp_id, "整列", |c| {
                edit::move_layer(c, &l.id, now, delta);
                Ok(String::new())
            })
        }
        Op::Marker { comment } => s.edit_comp(&comp_id, "マーカーを追加", |c| {
            if c.markers.iter().any(|m| m.time == now) {
                return Err("この時刻にはマーカーがあります".into());
            }
            c.markers.push(Marker { time: now, duration: Time::ZERO, comment });
            c.markers.sort_by_key(|m| m.time);
            Ok(String::new())
        }),
        Op::ClearMarkers => s.edit_comp(&comp_id, "マーカーをすべて削除", |c| {
            c.markers.clear();
            Ok("マーカーをすべて削除しました".into())
        }),
        Op::DetectBeats { layer, sensitivity } => {
            let m = match mono(s, &comp, layer.map(LayerId::new).as_ref()) {
                Ok(m) => m,
                Err(e) => fail!(format!("解析できません: {e}")),
            };
            let onsets = detect_onsets(&m, sensitivity);
            let n = onsets.len();
            s.edit_comp(&comp_id, "ビート検出", |c| {
                for t in onsets {
                    c.markers.push(Marker { time: beat::nearest_frame(Time::from_seconds(t), fr), duration: Time::ZERO, comment: String::new() });
                }
                c.markers.sort_by_key(|m| m.time);
                c.markers.dedup_by_key(|m| m.time);
                Ok(format!("{n} 個のビートを検出しました"))
            })
        }
        Op::BandScale { layer, source, band } => {
            let band = match band.as_str() {
                "low" => Band::LOW,
                "mid" => Band::MID,
                _ => Band::HIGH,
            };
            let m = match mono(s, &comp, source.map(LayerId::new).as_ref()) {
                Ok(m) => m,
                Err(e) => fail!(format!("解析できません: {e}")),
            };
            let keys = band_keyframes(&m, band, fr, Time::ZERO, comp.duration);
            s.edit_comp(&comp_id, "音量をキーフレーム化", |c| {
                let l = c.layer_mut(&LayerId::new(layer)).ok_or("レイヤーがありません")?;
                l.transform.scale = Property::constant([100.0, 100.0]).with_keys(
                    keys.keyframes
                        .iter()
                        .map(|k| {
                            let v = 100.0 + (k.value * 2.0).min(100.0) * 0.5;
                            Keyframe::linear(k.time, [v, v])
                        })
                        .collect(),
                );
                Ok("音量をスケールのキーフレームにしました".into())
            })
        }
        Op::BpmFromTaps => {
            let taps: Vec<Time> = comp.markers.iter().filter(|m| m.comment.is_empty()).map(|m| m.time).collect();
            let Some(g) = beat::grid_from_taps(&taps) else { fail!("空コメントのマーカーが 2 個以上必要です".into()) };
            s.edit_comp(&comp_id, "ビートグリッド", |c| {
                c.beat_grid = Some(g);
                Ok(format!("BPM {:.1}（オフセット {:.3} 秒）を設定しました", g.bpm, g.offset.seconds()))
            })
        }
        Op::LockComp { on } => {
            if let Some(owner) = s.locked_by_other(&comp_id) {
                fail!(format!("このコンポは {owner} がロック中です"));
            }
            let user = s.user.clone();
            let r = s.mutate(if on { "コンポをロック" } else { "コンポのロックを解除" }, |p| {
                p.comp_mut(&comp_id).ok_or("コンポがありません")?.locked_by = on.then_some(user);
                Ok(String::new())
            });
            OpResult { snapshot: r.snapshot, status: r.status, ..Default::default() }
        }
        Op::ReverseKeys { layer, keys: sel } => {
            let Some((lo, hi)) = keys::span(&sel) else { fail!(String::new()) };
            let mut out = Vec::new();
            let mut r = s.edit_comp(&comp_id, "キーフレームの時間反転", |c| {
                out = keys::reverse_between(c.layer_mut(&LayerId::new(layer)).ok_or("レイヤーがありません")?, &sel, lo, hi);
                Ok(String::new())
            });
            r.keys = Some(out);
            r
        }
        Op::ScaleKeys { layer, keys: sel, percent } => {
            let Some((lo, _)) = keys::span(&sel) else { fail!(String::new()) };
            if (percent / 100.0).abs() < 1e-6 {
                fail!(String::new());
            }
            let mut out = Vec::new();
            let mut r = s.edit_comp(&comp_id, "キーフレームを伸縮", |c| {
                out = keys::scale(c.layer_mut(&LayerId::new(layer)).ok_or("レイヤーがありません")?, &sel, lo, percent / 100.0, fr);
                Ok(String::new())
            });
            r.keys = Some(out);
            r
        }
        Op::CopyKeys { layer, keys: sel } => {
            let Ok(l) = layer_of(&layer) else { fail!("レイヤーがありません".into()) };
            let clip = keys::copy(&l, &sel);
            if clip.is_empty() {
                fail!(String::new());
            }
            OpResult { status: format!("キーフレームを {} 個コピーしました", clip.key_count()), clip: Some(clip.to_text()), ..Default::default() }
        }
        Op::PasteKeys { layer, text, target } => {
            let Some(clip) = KeyClipboard::from_text(&text) else { fail!(String::new()) };
            if let Err(e) = unlocked(&layer) {
                fail!(format!("{e}（貼り付けられません）"));
            }
            let mut pasted = Vec::new();
            let mut r = s.edit_comp(&comp_id, "キーフレームを貼り付け", |c| {
                pasted = keys::paste(c.layer_mut(&LayerId::new(layer)).ok_or("レイヤーがありません")?, &clip, now, target.as_deref());
                if pasted.is_empty() {
                    return Err("貼り付けられるプロパティがありません（型が違います）".into());
                }
                Ok(format!("キーフレームを {} 個貼り付けました", pasted.len()))
            });
            r.keys = Some(pasted);
            r
        }
        Op::Stamp { layer, source, range, restart } => {
            let id = LayerId::new(layer);
            let ((_, end), times) = beat_plan(&comp, source, range, &id);
            if times.is_empty() {
                fail!("対象のビートがありません".into());
            }
            let base = s.project.fresh_id("beat");
            let mut counter = 0;
            let mode = if restart { StampMode::Restart } else { StampMode::Continue };
            let mut r = s.edit_comp(&comp_id, "ビートに合わせて配置", |c| {
                let made = beatfit::stamp_on_beats(c, &id, &times, end, mode, &mut || {
                    counter += 1;
                    LayerId::new(format!("{base}-{counter}"))
                });
                Ok(format!("{} 個に並べました（⌘Z で元に戻せます）", made.len()))
            });
            r.keys = Some(Vec::new());
            r
        }
        Op::Pulse { layer, source, range, path, delta, frames } => {
            let id = LayerId::new(layer);
            let (_, times) = beat_plan(&comp, source, range, &id);
            if times.is_empty() {
                fail!("対象のビートがありません".into());
            }
            let len = fr.frame_to_time(frames.max(1) as i64);
            s.edit_comp(&comp_id, "ビートでパルス", |c| {
                let l = c.layer_mut(&id).ok_or("レイヤーがありません")?;
                if beatfit::pulse_on_beats(l, &path, &times, &[delta], len) == 0 {
                    return Err("このプロパティにはキーフレームを打てません".into());
                }
                Ok(format!("{} 個のビートにキーフレームを打ちました", times.len()))
            })
        }
        Op::AddMask { layer, ellipse } => {
            let Ok(l) = layer_of(&layer) else { fail!("レイヤーがありません".into()) };
            let size = s.project.source_size(&l.kind);
            let (w, h) = size.unwrap_or((comp.width as f64, comp.height as f64));
            // 寸法を持たないレイヤー（シェイプ・テキスト）は原点が中心
            let (ox, oy) = if size.is_none() { (-w / 2.0, -h / 2.0) } else { (0.0, 0.0) };
            let path = if ellipse {
                znd_core::shape::BezPath::ellipse([ox + w / 2.0, oy + h / 2.0], [w * 0.3, h * 0.3])
            } else {
                znd_core::shape::BezPath::rect(ox + w * 0.25, oy + h * 0.25, w * 0.5, h * 0.5)
            };
            s.edit_comp(&comp_id, "マスクを追加", |c| {
                let l = c.layer_mut(&LayerId::new(layer)).ok_or("レイヤーがありません")?;
                if l.locked {
                    return Err("ロックされています".into());
                }
                l.masks.push(znd_core::shape::Mask::new(path));
                Ok(format!("マスク {} を追加しました", l.masks.len()))
            })
        }
        Op::SyncTest => {
            let wav = crate::base_dir(s.path.as_deref()).join("znd-sync-test-120bpm.wav");
            let seconds = 8.0;
            if let Err(e) = znd_media::audio::write_wav(&wav, &znd_audio::analysis::click_track(120.0, seconds)) {
                fail!(format!("クリック音を書き出せません: {e}"));
            }
            let id = s.project.fresh_id("comp");
            let test = beatfit::sync_test_comp(&id, comp.width, comp.height, fr, 120.0, seconds);
            let cid = CompId::new(id.as_str());
            let r = s.mutate("同期テストのコンポ", |p| {
                p.compositions.push(test);
                znd_media::import_file(p, &cid, &wav, znd_media::ImportOptions::default()).map_err(|e| format!("クリック音を読み込めません: {e}"))?;
                Ok("同期テストのコンポを作りました。再生して、光と音がずれて感じられたら補正を調整してください".into())
            });
            let ok = r.snapshot.is_some();
            OpResult { snapshot: r.snapshot, status: r.status, comp: ok.then_some(id), ..Default::default() }
        }
    }
}

#[tauri::command]
pub fn op(state: State<'_, Shared>, ctx: OpCtx, op: Op) -> OpResult {
    layer_op(&mut lock(&state), &ctx, op)
}

/// ビートの取り方の一覧（画面の選択欄）。
#[tauri::command]
pub fn beat_sources() -> Vec<&'static str> {
    BEAT_SOURCES.to_vec()
}

/// ビートに合わせる操作の対象になるビートの数。
#[tauri::command]
pub fn beat_count(state: State<'_, Shared>, comp: String, layer: Option<String>, source: usize, range: Option<(i64, i64)>) -> usize {
    let s = lock(&state);
    let Some(c) = s.project.comp(&CompId::new(comp)) else { return 0 };
    beat_plan(c, source, range, &LayerId::new(layer.unwrap_or_default())).1.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn studio() -> Studio {
        let mut p = Project::new("t");
        let mut c = Composition::new("c", "c", 100, 100, FrameRate::FPS_30, Time::from_seconds(4.0));
        for id in ["a", "b"] {
            c.layers.push(Layer::new(id, id, LayerKind::Solid { width: 10, height: 10, color: [1.0; 3] }, Transform::centered(10.0, 10.0, 100.0, 100.0), c.duration));
        }
        p.compositions.push(c);
        Studio::new(p, None)
    }

    fn ctx(frame: i64) -> OpCtx {
        OpCtx { comp: "c".into(), frame }
    }

    fn comp(s: &Studio) -> &Composition {
        s.project.comp(&CompId::new("c")).unwrap()
    }

    #[test]
    fn split_duplicate_and_delete_select_the_result() {
        let mut s = studio();
        let r = layer_op(&mut s, &ctx(30), Op::Split { layer: "a".into() });
        let new = r.select.unwrap().unwrap();
        assert_eq!(comp(&s).layers.len(), 3);
        assert_eq!(comp(&s).layer(&LayerId::new(new.as_str())).unwrap().in_point, Time::from_seconds(1.0));
        let r = layer_op(&mut s, &ctx(0), Op::Duplicate { layer: "b".into() });
        assert!(r.select.unwrap().is_some());
        assert_eq!(comp(&s).layers.len(), 4);
        let r = layer_op(&mut s, &ctx(0), Op::Delete { layer: "b".into() });
        assert_eq!(r.select, Some(None));
        assert!(comp(&s).layer(&LayerId::new("b")).is_none());
    }

    #[test]
    fn parent_cycles_are_refused() {
        let mut s = studio();
        layer_op(&mut s, &ctx(0), Op::Parent { layer: "a".into(), parent: Some("b".into()) });
        let r = layer_op(&mut s, &ctx(0), Op::Parent { layer: "b".into(), parent: Some("a".into()) });
        assert!(r.snapshot.is_none());
        assert!(r.status.contains("循環"));
        assert_eq!(comp(&s).layer(&LayerId::new("b")).unwrap().parent, None);
    }

    #[test]
    fn comps_locked_by_others_are_not_edited() {
        let mut s = studio();
        s.user = "わたし".into();
        let mut p = (*s.project).clone();
        p.compositions[0].locked_by = Some("ほかの人".into());
        s.apply(p, "x", false);
        let r = layer_op(&mut s, &ctx(0), Op::Marker { comment: String::new() });
        assert!(r.snapshot.is_none());
        assert!(r.status.contains("ほかの人"));
    }

    #[test]
    fn keys_can_be_copied_and_pasted_through_text() {
        let mut s = studio();
        let mut p = (*s.project).clone();
        let l = p.compositions[0].layer_mut(&LayerId::new("a")).unwrap();
        l.transform.rotation = Property::constant(0.0).with_keys(vec![Keyframe::linear(Time::ZERO, 0.0), Keyframe::linear(Time::from_seconds(1.0), 90.0)]);
        s.apply(p, "x", false);
        let sel = vec![KeyAddr::new("transform.rotation", Time::ZERO), KeyAddr::new("transform.rotation", Time::from_seconds(1.0))];
        let r = layer_op(&mut s, &ctx(0), Op::CopyKeys { layer: "a".into(), keys: sel });
        let text = r.clip.unwrap();
        let r = layer_op(&mut s, &ctx(60), Op::PasteKeys { layer: "b".into(), text, target: None });
        assert_eq!(r.keys.unwrap().len(), 2);
        let b = comp(&s).layer(&LayerId::new("b")).unwrap();
        assert_eq!(b.transform.rotation.keyframes.len(), 2);
        assert_eq!(b.transform.rotation.keyframes[0].time, Time::from_seconds(2.0));
    }
}
