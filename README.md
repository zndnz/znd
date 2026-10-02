# znd

ネット発の自主制作映像クリエイター向け、レイヤーベースの合成・モーショングラフィックスエンジン。
要件は [PRODUCT.md](PRODUCT.md)。M1〜M5 を通して実装したが、要件のすべては満たしていない（GPU はエフェクトのみ、OpenFX は一部など）。
何ができて何ができないかは [docs/04-milestones.md](docs/04-milestones.md) にまとめてある。

## 設計ドキュメント

1. [全体アーキテクチャ](docs/01-architecture.md)
2. [データモデル](docs/02-data-model.md)
3. [評価パイプライン](docs/03-evaluation-pipeline.md)
4. [マイルストーン計画と実装状況](docs/04-milestones.md)
5. [受け入れ基準](docs/05-acceptance-criteria.md)
6. [プラグイン](docs/06-plugins.md)

## 必要なもの

- Rust 1.80 以降
- FFmpeg（`ffmpeg` / `ffprobe`。動画の読み込みと書き出しに使用。`ZND_FFMPEG_DIR` で場所を指定可）
- C コンパイラ（ビルド時に FBX 読み込みと OpenFX ホストの一部をコンパイルする）
- GPU は任意。あればエフェクトの処理に使う（`ZND_GPU=0` で無効）

## 使い方

```bash
cargo run --release -p znd-app
```

デモプロジェクトが開く。Space で再生、←/→ でコマ送り、M でマーカー、B/N でワークエリア、⌘Z / ⇧⌘Z で取り消し・やり直し、⌘S で保存。
ビューアでは V（選択・移動）/ G（ペン）/ R（矩形マスク）/ O（楕円マスク）、タイムラインでは ⌘ + ホイールで拡大、バーやキーフレームをドラッグして移動できる。

```bash
cargo run --release -p znd-cli -- demo demo.znd.json
```

```bash
cargo run --release -p znd-cli -- export demo.znd.json -o demo.mp4
```

提出前チェック（権利・欠落素材・エクスプレッション・明滅・ラウドネス）:

```bash
cargo run --release -p znd-cli -- check demo.znd.json
```

素材の読み込み（動画・静止画・連番・PSD・SVG/PDF/AI・3D モデル）とプラグインの一覧:

```bash
cargo run --release -p znd-cli -- import demo.znd.json 素材.psd
```

```bash
cargo run --release -p znd-cli -- plugins
```

その他のコマンド（歌詞の読み込み、ビート検出、版の比較、ファイル収集、スクリプト実行など）は `znd help`。

## テスト

```bash
cargo test --workspace
```

参照画像を更新するとき（差分を目視してからコミットすること）:

```bash
ZND_BLESS=1 cargo test -p znd-render
```

## 構成

| クレート | 内容 |
|---|---|
| `crates/znd-core` | データモデル、時刻、キーフレーム補間、プロジェクト I/O、権利・共同制作・制作ログ |
| `crates/znd-text` | テキストレイアウト（シェーピング、縦書き、ルビ） |
| `crates/znd-render` | CPU レンダラー（合成、シェイプ、マスク、エフェクト、3D、品質チェック） |
| `crates/znd-audio` | ミキシング、タイムストレッチ、音声解析、ラウドネス |
| `crates/znd-script` | エクスプレッションと自動化スクリプト（QuickJS） |
| `crates/znd-gpu` | GPU バックエンド（wgpu。エフェクトの処理） |
| `crates/znd-media` | FFmpeg 連携（読み込み・各形式への書き出し）、素材管理 |
| `crates/znd-cli` | `znd` コマンド |
| `crates/znd-app` | アプリ |
| `sdk/`, `examples/plugins/` | プラグインの SDK ヘッダーとサンプル |

ビルド成果物を小さく保つため、デバッグ情報とインクリメンタルビルドは切ってある（[Cargo.toml](Cargo.toml)）。
