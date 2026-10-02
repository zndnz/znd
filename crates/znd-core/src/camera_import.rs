//! 外部 3DCG ソフトのカメラデータ取り込み（.chan 形式）。
//!
//! .chan は 1 行 1 フレームのテキスト: `フレーム番号 tx ty tz rx ry rz [垂直画角]`。
//! Blender・Maya・Houdini・Nuke から書き出せる。座標系は Y 上・右手系、カメラは -Z 方向を向く。
//! これを znd の座標系（Y 下・Z 奥）に直し、1 ノードカメラのキーフレームにする。

use crate::model::{Layer, LayerKind, Transform};
use crate::property::{Keyframe, Property};
use crate::time::{FrameRate, Time};

/// `.chan` を解釈してカメラレイヤーを作る。
///
/// - `px_per_unit`: 3D ソフトの 1 単位を何 px とするか。
/// - `default_vfov`: ファイルに画角の列が無いときの垂直画角（度）。
pub fn parse_chan(src: &str, comp_w: u32, comp_h: u32, fr: FrameRate, px_per_unit: f64, default_vfov: f64) -> Result<Layer, String> {
    let (cx, cy) = (comp_w as f64 / 2.0, comp_h as f64 / 2.0);
    let mut pos = Vec::new();
    let mut pos_z = Vec::new();
    let (mut rx, mut ry, mut rz) = (Vec::new(), Vec::new(), Vec::new());
    let mut zoom = Vec::new();
    let mut first_frame: Option<i64> = None;
    for (n, line) in src.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let v: Vec<f64> = line
            .split_whitespace()
            .map(|s| s.parse::<f64>())
            .collect::<Result<_, _>>()
            .map_err(|_| format!("{} 行目: 数値として読めません", n + 1))?;
        if v.len() < 7 {
            return Err(format!("{} 行目: 列が足りません（フレーム tx ty tz rx ry rz が必要）", n + 1));
        }
        let frame = v[0].round() as i64;
        // 最初のフレームをコンポの先頭に合わせる
        let t = fr.frame_to_time(frame - *first_frame.get_or_insert(frame));
        // Y と Z の向きを反転する（X 軸まわりに 180° 回した座標系）
        pos.push(Keyframe::linear(t, [cx + v[1] * px_per_unit, cy - v[2] * px_per_unit]));
        pos_z.push(Keyframe::linear(t, -v[3] * px_per_unit));
        // 上を向く回転（+rx）は、こちらでは X 回転の負の向き。ヨーは同じ向き、ロールは逆向き
        rx.push(Keyframe::linear(t, -v[4]));
        ry.push(Keyframe::linear(t, v[5]));
        rz.push(Keyframe::linear(t, -v[6]));
        let vfov = v.get(7).copied().unwrap_or(default_vfov).clamp(1.0, 179.0);
        zoom.push(Keyframe::linear(t, cy / (vfov.to_radians() / 2.0).tan()));
    }
    let Some(last) = pos.last().map(|k| k.time) else { return Err("カメラデータがありません".into()) };
    let mut tr = Transform::centered(0.0, 0.0, comp_w as f64, comp_h as f64);
    tr.position = Property::spatial([cx, cy]).with_keys(pos);
    tr.position_z = Property::constant(0.0).with_keys(pos_z);
    tr.rotation_x = Property::constant(0.0).with_keys(rx);
    tr.rotation_y = Property::constant(0.0).with_keys(ry);
    tr.rotation = Property::constant(0.0).with_keys(rz);
    let z0 = zoom[0].value;
    let kind = LayerKind::Camera {
        zoom: Property::constant(z0).with_keys(zoom),
        point_of_interest: None,
        depth_of_field: false,
        focus_distance: Property::constant(z0),
        aperture: Property::constant(0.0),
        iris_sides: 0,
    };
    Ok(Layer::new("camera-import", "取り込みカメラ", kind, tr, last + fr.frame_to_time(1)))
}

/// 既定のカメラレイヤー（50mm 相当、Z=0 の面が等倍）。
pub fn default_camera(id: &str, comp_w: u32, comp_h: u32, duration: Time) -> Layer {
    let zoom = comp_w as f64 * 50.0 / 36.0;
    let mut tr = Transform::centered(0.0, 0.0, comp_w as f64, comp_h as f64);
    tr.position_z = Property::constant(-zoom);
    let kind = LayerKind::Camera {
        zoom: Property::constant(zoom),
        point_of_interest: Some(Property::constant([comp_w as f64 / 2.0, comp_h as f64 / 2.0, 0.0])),
        depth_of_field: false,
        focus_distance: Property::constant(zoom),
        aperture: Property::constant(25.0),
        iris_sides: 0,
    };
    Layer::new(id, "カメラ", kind, tr, duration)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chan_is_converted_to_znd_coordinates() {
        // 3 フレーム: 原点の手前 5 単位から、右へ 1 単位動きながら右を向く。画角 90° → 60°
        let src = "# camera\n101 0 0 5 0 0 0 90\n102 0.5 1 5 10 -15 5 90\n103 1 2 5 20 -30 10 60\n";
        let l = parse_chan(src, 1920, 1080, FrameRate::FPS_24, 100.0, 40.0).unwrap();
        let LayerKind::Camera { zoom, point_of_interest, .. } = &l.kind else { panic!() };
        assert!(point_of_interest.is_none());
        let t = |f: i64| FrameRate::FPS_24.frame_to_time(f);
        // 最初のフレームがコンポ先頭
        assert_eq!(l.transform.position.eval(t(0)), [960.0, 540.0]);
        assert_eq!(l.transform.position_z.eval(t(0)), -500.0);
        // Y 上 → Y 下: +2 単位は画面上方向（y が減る）
        assert_eq!(l.transform.position.eval(t(2)), [1060.0, 340.0]);
        assert_eq!(l.transform.rotation_x.eval(t(2)), -20.0);
        assert_eq!(l.transform.rotation_y.eval(t(2)), -30.0);
        assert_eq!(l.transform.rotation.eval(t(2)), -10.0);
        // 垂直画角 90° → ズーム = 高さ/2、60° → 高さ/2 ÷ tan30°
        assert!((zoom.eval(t(0)) - 540.0).abs() < 1e-9);
        assert!((zoom.eval(t(2)) - 540.0 / (30f64.to_radians()).tan()).abs() < 1e-9);
        assert_eq!(l.out_point, t(3));
        // 画角の列が無ければ既定値
        let l = parse_chan("1 0 0 5 0 0 0\n", 1920, 1080, FrameRate::FPS_24, 100.0, 90.0).unwrap();
        let LayerKind::Camera { zoom, .. } = &l.kind else { panic!() };
        assert!((zoom.eval(Time::ZERO) - 540.0).abs() < 1e-9);
        assert!(parse_chan("", 1920, 1080, FrameRate::FPS_24, 100.0, 40.0).is_err());
        assert!(parse_chan("1 2 3\n", 1920, 1080, FrameRate::FPS_24, 100.0, 40.0).unwrap_err().contains("1 行目"));
        assert!(parse_chan("1 a 0 0 0 0 0\n", 1920, 1080, FrameRate::FPS_24, 100.0, 40.0).is_err());
    }
}
