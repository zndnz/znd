fn main() {
    // 画面（studio/）をまだビルドしていなくても Rust 側だけはビルドできるよう、仮の index.html を置く。
    // `cargo build --workspace` で、画面のビルドを必須にしないため。
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../studio/dist");
    if !dist.join("index.html").exists() {
        let _ = std::fs::create_dir_all(&dist);
        let _ = std::fs::write(
            dist.join("index.html"),
            "<!doctype html><meta charset=utf-8><body style=\"background:#000;color:#aaa;font:14px system-ui;padding:2em\">\
             画面がビルドされていません。<code>cd studio && npm install && npm run build</code> を実行してください。",
        );
    }
    tauri_build::build()
}
