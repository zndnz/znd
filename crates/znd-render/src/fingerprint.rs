//! フレームの指紋（内容ハッシュ）。プレビューキャッシュとディスクキャッシュのキーに使う。
//!
//! 「その時刻の絵を決めるもの」だけからハッシュを作るので、編集しても影響の無いフレームは
//! キャッシュが生き残る。たとえば 10 秒目から始まるレイヤーを編集しても、0〜10 秒のフレームは
//! 再レンダリングされない。静止しているレイヤーは時刻をハッシュに含めないので、
//! 絵が変わらない区間は同じ指紋になり、1 枚のキャッシュを共有する。

use crate::compositor::RenderOptions;
use std::collections::HashMap;
use znd_core::effect::{effect_def, ParamValue};
use znd_core::eval::layer_source_seconds;
use znd_core::shape::ShapeItem;
use znd_core::visit::visit_layer;
use znd_core::{CompId, Composition, Layer, LayerKind, Project, TextSelector, Time};

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// プロセスや Rust のバージョンに依らない安定したハッシュ（ディスクキャッシュのファイル名になる）。
fn fnv(mut h: u64, bytes: &[u8]) -> u64 {
    for b in bytes {
        h = (h ^ *b as u64).wrapping_mul(FNV_PRIME);
    }
    h
}

fn mix(h: u64, v: u64) -> u64 {
    fnv(h, &v.to_le_bytes())
}

struct LayerInfo {
    /// レイヤーの定義そのもののハッシュ。
    hash: u64,
    /// 定義が同じでも時刻によって絵が変わるか。
    varying: bool,
}

/// プロジェクト 1 リビジョンぶんの前計算。編集のたびに作り直す（JSON 化 1 回ぶんの軽い処理）。
pub struct Fingerprinter {
    settings: u64,
    layers: HashMap<(usize, usize), LayerInfo>,
}

fn shape_wiggles(items: &[ShapeItem]) -> bool {
    items.iter().any(|i| match i {
        ShapeItem::WigglePath { speed, .. } => speed.is_animated() || speed.value != 0.0,
        ShapeItem::Group { items, .. } => shape_wiggles(items),
        _ => false,
    })
}

fn is_varying(layer: &Layer) -> bool {
    let mut animated = false;
    visit_layer(layer, &mut |_, p| animated |= !p.key_times().is_empty() || p.has_expression());
    if animated || layer.motion_blur {
        return true;
    }
    if layer.effects.iter().any(|e| e.enabled && effect_def(&e.kind).is_some_and(|d| d.time_dependent)) {
        return true;
    }
    match &layer.kind {
        // 動画フッテージとプリコンポは、下で「ソースのどのフレームか」を別途ハッシュに入れる
        LayerKind::Shape { items } => shape_wiggles(items),
        LayerKind::Text { animators, .. } => {
            animators.iter().any(|a| a.selectors.iter().any(|s| matches!(s, TextSelector::Wiggly { .. } | TextSelector::Expression { .. })))
        }
        _ => false,
    }
}

impl Fingerprinter {
    pub fn new(project: &Project) -> Self {
        let settings = fnv(FNV_OFFSET, serde_json::to_string(&project.settings).unwrap_or_default().as_bytes());
        let mut layers = HashMap::new();
        for (ci, comp) in project.compositions.iter().enumerate() {
            for (li, layer) in comp.layers.iter().enumerate() {
                let mut hash = fnv(FNV_OFFSET, serde_json::to_string(layer).unwrap_or_default().as_bytes());
                // フッテージは「どのファイルの、いつの版か」も絵を決める
                if let LayerKind::Footage { footage } | LayerKind::Model { footage } = &layer.kind {
                    if let Some(item) = project.footage_item(footage) {
                        hash = fnv(hash, item.path.as_bytes());
                        hash = mix(hash, ((item.width as u64) << 32) | item.height as u64);
                        let mtime = std::fs::metadata(&item.path).and_then(|m| m.modified()).ok();
                        if let Some(d) = mtime.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()) {
                            hash = mix(hash, d.as_secs());
                        }
                    }
                }
                // Alembic はファイルの中に時間変化を持ちうる
                let abc = match &layer.kind {
                    LayerKind::Model { footage } => project.footage_item(footage).is_some_and(|f| f.path.to_lowercase().ends_with(".abc")),
                    _ => false,
                };
                layers.insert((ci, li), LayerInfo { hash, varying: abc || is_varying(layer) });
            }
        }
        Fingerprinter { settings, layers }
    }

    /// コンポ `comp` の時刻 `t` のフレームの指紋。`project` は `new` に渡したものと同じ内容であること。
    pub fn frame(&self, project: &Project, comp: &CompId, t: Time, opts: RenderOptions) -> u64 {
        let mut h = mix(self.settings, opts.downsample as u64 | (opts.background as u64) << 8 | (opts.guides as u64) << 9);
        // プラグインの出力フィルター・エクスプレッション用ライブラリが変わったら別の絵として扱う
        h = mix(h, crate::plugins::hooks::fingerprint(opts.guides));
        match project.compositions.iter().position(|c| &c.id == comp) {
            Some(ci) => self.comp(project, ci, t, mix(h, 1), 0),
            None => {
                h = mix(h, 0);
                h
            }
        }
    }

    fn comp(&self, project: &Project, ci: usize, t: Time, mut h: u64, depth: usize) -> u64 {
        let comp: &Composition = &project.compositions[ci];
        h = fnv(h, comp.id.0.as_bytes());
        h = mix(h, ((comp.width as u64) << 32) | comp.height as u64);
        h = mix(h, ((comp.frame_rate.num as u64) << 32) | comp.frame_rate.den as u64);
        h = fnv(h, &comp.background.iter().flat_map(|c| c.to_le_bytes()).collect::<Vec<u8>>());
        if comp.motion_blur.enabled {
            h = mix(
                h,
                comp.motion_blur.shutter_angle.to_bits()
                    ^ comp.motion_blur.shutter_phase.to_bits().rotate_left(17)
                    ^ comp.motion_blur.samples as u64,
            );
        }
        if depth > 32 {
            return h;
        }
        // ソロの有無はコンポ全体の見え方を変える
        h = mix(h, comp.layers.iter().any(|l| l.solo) as u64);
        // カメラやライトが動いていれば、3D レイヤーすべての見え方が毎フレーム変わる
        let scene_varies = comp.layers.iter().enumerate().any(|(li, l)| {
            matches!(l.kind, LayerKind::Camera { .. } | LayerKind::Light { .. }) && l.active_at(t) && self.layers[&(ci, li)].varying
        });

        for (li, layer) in comp.layers.iter().enumerate() {
            // この時刻に存在しないレイヤーは絵に関与しない（他レイヤーの親としては下で拾う）
            if !layer.active_at(t) {
                continue;
            }
            let info = &self.layers[&(ci, li)];
            h = mix(h, info.hash);
            let mut varying = info.varying || (scene_varies && layer.three_d);

            // 親チェーン: 親が動けば子も動く。親自身がこの時刻に存在しなくても、その変形は効く
            let mut parent = layer.parent.as_ref();
            for _ in 0..comp.layers.len() {
                let Some(pi) = parent.and_then(|id| comp.layer_index(id)) else { break };
                let pinfo = &self.layers[&(ci, pi)];
                h = mix(h, pinfo.hash);
                varying |= pinfo.varying;
                parent = comp.layers[pi].parent.as_ref();
            }
            // エフェクトが参照するレイヤー（マップ等）と、その動き
            for fx in layer.effects.iter().filter(|e| e.enabled) {
                for v in fx.params.values() {
                    if let ParamValue::Layer(Some(id)) = v {
                        if let Some(ri) = comp.layer_index(id) {
                            h = mix(h, self.layers[&(ci, ri)].hash);
                            varying |= self.layers[&(ci, ri)].varying
                                || matches!(comp.layers[ri].kind, LayerKind::Footage { .. } | LayerKind::Precomp { .. });
                        }
                    }
                }
            }

            match &layer.kind {
                LayerKind::Footage { footage } => {
                    // 動画は「ソースの何フレーム目か」で決まる（同じフレームを保持している間は同じ指紋）
                    if let Some(fr) = project.footage_item(footage).and_then(|f| f.frame_rate) {
                        let seconds = layer_source_seconds(comp, layer, t);
                        let frame =
                            if layer.frame_blend { (seconds * fr.fps() * 256.0).floor() } else { (seconds * fr.fps() + 1e-6).floor() };
                        h = mix(h, frame as i64 as u64);
                    }
                }
                LayerKind::Precomp { comp: nested } => {
                    if let Some(ni) = project.compositions.iter().position(|c| &c.id == nested) {
                        let nc = &project.compositions[ni];
                        let local = Time::from_seconds(layer_source_seconds(comp, layer, t));
                        if local >= Time::ZERO && local < nc.duration {
                            let nested_t = nc.frame_rate.frame_to_time(nc.frame_rate.time_to_frame(local));
                            h = self.comp(project, ni, nested_t, h, depth + 1);
                        }
                    }
                }
                _ => {}
            }
            if varying {
                h = mix(h, t.0 as u64);
            }
        }
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use znd_core::demo::demo_project;
    use znd_core::{LayerId, Property};

    const OPTS: RenderOptions = RenderOptions::PREVIEW;

    fn main() -> CompId {
        CompId::new("main")
    }

    fn fp(p: &Project, seconds: f64) -> u64 {
        Fingerprinter::new(p).frame(p, &main(), Time::from_seconds(seconds), OPTS)
    }

    #[test]
    fn same_project_same_fingerprint_and_options_matter() {
        let p = demo_project();
        assert_eq!(fp(&p, 1.0), fp(&p.clone(), 1.0));
        assert_ne!(fp(&p, 1.0), fp(&p, 1.5), "アニメーションしているコンポは時刻で変わる");
        let f = Fingerprinter::new(&p);
        let t = Time::from_seconds(1.0);
        assert_ne!(f.frame(&p, &main(), t, OPTS), f.frame(&p, &main(), t, RenderOptions { downsample: 2, ..OPTS }));
        assert_ne!(f.frame(&p, &main(), t, OPTS), f.frame(&p, &main(), t, RenderOptions::EXPORT));
    }

    #[test]
    fn editing_a_layer_only_invalidates_frames_where_it_is_active() {
        let mut p = demo_project();
        // 「衛星 A」を 3 秒目からに限定する
        let layer = p.compositions[0].layer_mut(&LayerId::new("sat-a")).unwrap();
        layer.in_point = Time::from_seconds(3.0);
        let before = (fp(&p, 1.0), fp(&p, 4.0));
        // そのレイヤーの色を変える
        if let LayerKind::Solid { color, .. } = &mut p.compositions[0].layer_mut(&LayerId::new("sat-a")).unwrap().kind {
            *color = [0.0, 1.0, 0.0];
        }
        let after = (fp(&p, 1.0), fp(&p, 4.0));
        assert_eq!(before.0, after.0, "レイヤーが存在しない時刻のフレームは無効化されない");
        assert_ne!(before.1, after.1, "レイヤーが存在する時刻のフレームは無効化される");
    }

    #[test]
    fn static_comp_shares_one_fingerprint_across_time() {
        let mut p = demo_project();
        // 静止した平面だけのコンポ
        let comp = &mut p.compositions[0];
        comp.layers.retain(|l| l.id.0 == "bg");
        assert_eq!(fp(&p, 0.0), fp(&p, 3.0));
        // 不透明度にキーを打つと時刻で変わるようになる
        let bg = p.compositions[0].layer_mut(&LayerId::new("bg")).unwrap();
        bg.transform.opacity = Property::constant(100.0);
        bg.transform.opacity.set_key(Time::ZERO, 0.0);
        bg.transform.opacity.set_key(Time::from_seconds(5.0), 100.0);
        assert_ne!(fp(&p, 0.0), fp(&p, 3.0));
    }

    #[test]
    fn nested_comp_edits_propagate_and_parents_count() {
        let mut p = demo_project();
        let before = fp(&p, 1.0);
        // ネストしたコンポ（カラーバー）の中身を変える
        p.compositions[1].layers[0].transform.opacity = Property::constant(50.0);
        assert_ne!(before, fp(&p, 1.0));
        // 親（軌道ヌル）の回転を変えると、子（衛星）のあるフレームが変わる
        let mut q = demo_project();
        let b = fp(&q, 1.0);
        q.compositions[0].layer_mut(&LayerId::new("orbit")).unwrap().transform.rotation.keyframes[1].value = 720.0;
        assert_ne!(b, fp(&q, 1.0));
        // 関係のない別コンポを足しても変わらない
        let mut r = demo_project();
        let b = fp(&r, 1.0);
        r.compositions.push(Composition::new("other", "other", 10, 10, znd_core::FrameRate::FPS_30, Time::from_seconds(1.0)));
        assert_eq!(b, fp(&r, 1.0));
    }
}
