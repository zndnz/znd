//! FBX と Alembic の読み込み。素材は Blender 5.2 で書き出したもの（tests/fixtures）:
//! 一辺 2 の立方体（Blender 座標 (1,2,3)、マテリアル色 (0.8,0.1,0.05)）、4x4 の平面、35mm のカメラ。
//! `anim.abc` は 24fps の 1〜5 フレームで、立方体が X に +4、カメラが上に +4 動く。

use std::path::{Path, PathBuf};
use znd_render::alembic::Archive;
use znd_render::model::{load_model, load_model_at, Model};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn bounds(m: &Model) -> ([f64; 3], [f64; 3]) {
    let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
    for p in m.meshes.iter().flat_map(|m| &m.positions) {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo, hi)
}

fn close(a: [f64; 3], b: [f64; 3], tol: f64) {
    assert!((0..3).all(|k| (a[k] - b[k]).abs() <= tol), "{a:?} != {b:?}");
}

/// Blender（Z 上）の (1,2,3) の立方体と XY 平面は、Y 上では立方体が (1,3,-2)、平面が XZ 面。
fn check_static_scene(m: &Model) {
    let tris: usize = m.meshes.iter().map(|m| m.indices.len() / 3).sum();
    assert_eq!(tris, 14, "立方体 12 + 平面 2");
    let (lo, hi) = bounds(m);
    close(lo, [-2.0, 0.0, -3.0], 1e-4);
    close(hi, [2.0, 4.0, 2.0], 1e-4);
    close(m.center, [0.0, 2.0, -0.5], 1e-4);
    assert!((m.extent - 5.0).abs() < 1e-4);
    for mesh in &m.meshes {
        assert_eq!(mesh.normals.len(), mesh.positions.len());
        assert!(mesh.normals.iter().all(|n| ((n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() - 1.0).abs() < 1e-3));
    }
}

#[test]
fn m1_fbx_from_blender_loads_with_materials() {
    let m = load_model(&fixture("scene.fbx")).unwrap();
    check_static_scene(&m);
    // 立方体のマテリアル色
    let red = m.meshes.iter().find(|m| m.indices.len() == 36).expect("立方体");
    let c = red.material.base_color;
    assert!((c[0] - 0.8).abs() < 0.01 && (c[1] - 0.1).abs() < 0.01 && (c[2] - 0.05).abs() < 0.01, "{c:?}");
    // 面の向き: 立方体の法線は中心から外を向く
    for (p, n) in red.positions.iter().zip(&red.normals) {
        let out = [p[0] - 1.0, p[1] - 3.0, p[2] + 2.0];
        assert!(out[0] * n[0] + out[1] * n[1] + out[2] * n[2] > 0.0);
    }
}

#[test]
fn m2_alembic_static_mesh_and_camera() {
    let m = load_model(&fixture("scene.abc")).unwrap();
    check_static_scene(&m);
    let a = Archive::open(&fixture("scene.abc")).unwrap();
    let s = a.scene_at(0.0).unwrap();
    assert!(!s.animated);
    assert_eq!(s.cameras.len(), 1);
    let cam = &s.cameras[0];
    // Blender (0,-10,2) で +Y を向くカメラ → Y 上では (0,2,10) で -Z を向く
    close(cam.position(), [0.0, 2.0, 10.0], 1e-4);
    close(cam.forward(), [0.0, 0.0, -1.0], 1e-4);
    close(cam.up(), [0.0, 1.0, 0.0], 1e-4);
    assert!((cam.focal_length_mm - 35.0).abs() < 1e-6);
    assert!((cam.horizontal_aperture_mm - 36.0).abs() < 1e-3, "{}", cam.horizontal_aperture_mm);
    // 三角形の巻き方向が反時計回りに直っている（計算した面法線が外向き）
    let cube = s.meshes.iter().find(|m| m.indices.len() == 36).unwrap();
    for t in cube.indices.chunks_exact(3) {
        let (a, b, c) = (cube.positions[t[0] as usize], cube.positions[t[1] as usize], cube.positions[t[2] as usize]);
        let (u, v) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
        let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        let out = [a[0] - 1.0, a[1] - 3.0, a[2] + 2.0];
        assert!(n[0] * out[0] + n[1] * out[1] + n[2] * out[2] > 0.0);
    }
}

#[test]
fn m3_alembic_animation_is_sampled_and_interpolated() {
    let path = fixture("anim.abc");
    let a = Archive::open(&path).unwrap();
    let s = a.scene_at(0.0).unwrap();
    assert!(s.animated);
    assert!((s.start - 1.0 / 24.0).abs() < 1e-9 && (s.end - 5.0 / 24.0).abs() < 1e-9, "{} {}", s.start, s.end);
    let cube_x = |seconds: f64| {
        let s = a.scene_at(seconds).unwrap();
        let cube = s.meshes.iter().find(|m| m.indices.len() == 36).unwrap();
        cube.positions.iter().map(|p| p[0]).sum::<f64>() / cube.positions.len() as f64
    };
    assert!((cube_x(0.0) - 1.0).abs() < 1e-4);
    assert!((cube_x(3.0 / 24.0) - 3.0).abs() < 1e-4);
    // サンプルの間は線形補間
    let (x3, x4) = (cube_x(3.0 / 24.0), cube_x(4.0 / 24.0));
    assert!(x4 > x3 + 0.5);
    assert!((cube_x(3.5 / 24.0) - (x3 + x4) / 2.0).abs() < 1e-4);
    assert!((cube_x(10.0) - 5.0).abs() < 1e-4);
    let cam_y = |seconds: f64| a.scene_at(seconds).unwrap().cameras[0].position()[1];
    assert!((cam_y(1.0 / 24.0) - 2.0).abs() < 1e-4);
    assert!((cam_y(5.0 / 24.0) - 6.0).abs() < 1e-4);

    // レンダラーが使う入口: 時刻ごとに形が変わり、基準（中心・大きさ）は最初の状態のまま
    let first = load_model_at(&path, 1.0 / 24.0).unwrap();
    let last = load_model_at(&path, 5.0 / 24.0).unwrap();
    assert!((bounds(&last).1[0] - 6.0).abs() < 1e-4);
    assert_eq!(first.center, last.center);
    assert_eq!(first.extent, last.extent);
}

#[test]
fn m4_unsupported_files_are_explained() {
    let dir = std::env::temp_dir().join(format!("znd-models-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let hdf = dir.join("old.abc");
    std::fs::write(&hdf, b"\x89HDF\r\n\x1a\n................").unwrap();
    assert!(load_model(&hdf).unwrap_err().contains("HDF5"));
    let bad = dir.join("bad.fbx");
    std::fs::write(&bad, b"not an fbx").unwrap();
    assert!(load_model(&bad).unwrap_err().contains("FBX"));
}
