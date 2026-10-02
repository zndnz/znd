//! 同梱・利用している OSS とそのライセンス。About 画面と `znd licenses` で表示する。
//!
//! 依存を追加・更新したらこの表も更新すること（リリース前に `cargo tree` と突き合わせる）。

pub struct Component {
    pub name: &'static str,
    pub license: &'static str,
    pub usage: &'static str,
}

pub const COMPONENTS: &[Component] = &[
    Component { name: "serde / serde_json", license: "MIT OR Apache-2.0", usage: "プロジェクトファイル（JSON）の入出力" },
    Component { name: "rayon", license: "MIT OR Apache-2.0", usage: "並列レンダリング" },
    Component { name: "thiserror", license: "MIT OR Apache-2.0", usage: "エラー型" },
    Component {
        name: "image / png / zune-jpeg",
        license: "MIT OR Apache-2.0 (zune-jpeg: MIT OR Apache-2.0 OR Zlib)",
        usage: "静止画の読み書き",
    },
    Component { name: "egui / eframe", license: "MIT OR Apache-2.0", usage: "プレビューアプリの UI" },
    Component { name: "rfd", license: "MIT", usage: "ファイルダイアログ" },
    Component {
        name: "rustybuzz / ttf-parser", license: "MIT / MIT OR Apache-2.0", usage: "テキストのシェーピングと字形アウトライン"
    },
    Component { name: "fontdb", license: "MIT", usage: "システムフォントの検索" },
    Component {
        name: "rquickjs / QuickJS", license: "MIT", usage: "エクスプレッションと自動化スクリプトの JavaScript エンジン"
    },
    Component { name: "cpal", license: "Apache-2.0", usage: "音声の再生" },
    Component { name: "gltf", license: "MIT OR Apache-2.0", usage: "glTF / GLB モデルの読み込み" },
    Component { name: "libloading", license: "ISC", usage: "外部プラグイン（動的ライブラリ）の読み込み" },
    Component { name: "wgpu / naga", license: "MIT OR Apache-2.0", usage: "GPU でのエフェクト処理" },
    Component { name: "pollster / bytemuck", license: "MIT OR Apache-2.0（bytemuck は Zlib も可）", usage: "GPU の初期化とデータ受け渡し" },
    Component { name: "usvg / tiny-skia-path", license: "Apache-2.0 OR MIT / BSD-3-Clause", usage: "SVG の読み込み" },
    Component { name: "lopdf", license: "MIT", usage: "PDF・PDF 互換 AI の読み込み" },
    Component { name: "flate2", license: "MIT OR Apache-2.0", usage: "PSD の ZIP 圧縮レイヤーの展開" },
    Component { name: "ufbx", license: "MIT OR Unlicense", usage: "FBX モデルの読み込み" },
    Component {
        name: "OpenFX（API 定義）",
        license: "BSD-3-Clause",
        usage: "OpenFX プラグインのホスト。API の仕様に従って独自に実装（公式ヘッダーはテストでのみ使用）",
    },
    Component { name: "exr（image 経由）", license: "BSD-3-Clause", usage: "OpenEXR 連番の読み書き" },
    Component {
        name: "FFmpeg（外部プロセス）",
        license: "LGPL-2.1+ / GPL-2+（ビルド構成による）",
        usage: "動画のデコード・エンコード。リンクせず別プロセスとして起動するため、本体と作品のライセンスには影響しない",
    },
    Component {
        name: "x264 / x265（FFmpeg 経由）",
        license: "GPL-2+",
        usage: "H.264 / H.265 エンコード。FFmpeg プロセス内でのみ動作。生成した映像の利用に GPL の制約は及ばない",
    },
];

pub const NOTICE: &str = "本ツールで制作した作品は、商用・非商用を問わず本ツール自体のライセンスによる制約を受けません。\
H.264 等のコーデック特許については、配信形態によって別途ライセンスが必要になる場合があります。\
フォントは OS にインストールされたものを使います。各フォントの利用条件（映像作品への使用可否）は別途ご確認ください。\
プラグイン（znd ネイティブ / OpenFX）は別の著作物です。利用条件は各プラグインのライセンスに従ってください（ウィンドウ → プラグインで確認できます）。";
