# 6. プラグイン

エフェクトを外部の動的ライブラリで追加できる。2 種類に対応している。

| 種類 | エフェクトの種類名 | 形式 |
|---|---|---|
| znd ネイティブ | `plugin:<ID>` | `.dylib` / `.so` / `.dll`（[sdk/znd_plugin.h](../sdk/znd_plugin.h)） |
| OpenFX | `ofx:<識別子>` | `.ofx.bundle`（OFX 1.4 の画像エフェクト） |

読み込んだエフェクトは組み込みと同じ定義表に入るので、エフェクトの「追加」メニューに並び、
パラメータにキーフレームやエクスプレッションを付けられ、プリセットにも保存できる。

**プラグインはアプリと同じプロセス・同じ権限で動く。信頼できるものだけを入れること。**
受け取ったプロジェクトを開いただけでコードが実行されないよう、プロジェクトファイルの隣のフォルダは探さない。

## 置き場所

1. 環境変数 `ZND_PLUGIN_PATH` に並べたフォルダ
2. macOS: `~/Library/Application Support/znd/plugins`、Linux: `~/.local/share/znd/plugins`、Windows: `%APPDATA%\znd\plugins`
3. OpenFX の標準の場所（`OFX_PLUGIN_PATH`、macOS は `/Library/OFX/Plugins`）

アプリでは「ウィンドウ → プラグイン」で一覧・提供元・ライセンス・読み込みエラーを確認でき、
その場でファイルを読み込むこともできる（その起動の間だけ有効）。CLI では:

```bash
cargo run -p znd-cli -- plugins
```

## プラグインが無い環境で開いたとき

プロジェクトにはエフェクトの種類名とパラメータだけが保存される。プラグインが入っていない環境では
そのエフェクトは何もせず（元の画像のまま）、パラメータは消えずに残る。`znd check` と
プラグインウィンドウが、足りないプラグインと使っているレイヤーを列挙する。

## znd ネイティブプラグインを作る

`znd_plugin_describe()` で定義（エフェクトとパラメータ）を返し、`znd_plugin_render()` で
RGBA の float バッファ（リニアライト・プリマルチプライド）をその場で書き換える。
パラメータは Float / Bool / Choice / Color / Point の 5 種類で、ホストがキーフレームと
エクスプレッションを評価した「その時刻の値」を渡す。px 単位の値と位置は、プレビュー解像度に
合わせてホストが縮尺を掛けてから渡すので、プラグイン側で解像度を気にする必要はない。

サンプル:

- [examples/plugins/vignette.c](../examples/plugins/vignette.c)（C。周辺減光と色かぶり）
- [examples/plugins/scanline.rs](../examples/plugins/scanline.rs)（Rust。時刻で変わるエフェクト）

```bash
cc -O2 -shared -fPIC -Isdk -o vignette.dylib examples/plugins/vignette.c
```

描画が 0 以外を返すか、結果に NaN / 無限大が含まれていた場合、ホストは元の画像に戻して続ける。
`ZND_EFFECT_THREAD_SAFE` を付けていないエフェクトは、ホストが 1 スレッドずつ呼ぶ。

## OpenFX

ホストが実装しているもの:

- スイート: Property / ImageEffect / Parameter / Memory / MultiThread / Message（V1・V2）
- コンテキスト: Filter / General（入力は `Source` 1 本）
- 画素形式: RGBA の float（優先）、16bit、8bit。整数しか扱えないプラグインには表示値に変換して渡す
- パラメータ: Double / Integer / Boolean / Choice / RGB / RGBA / Double2D / Integer2D / Double3D / Integer3D / String、
  別時刻の値（`paramGetValueAtTime`）、微分・積分
- 別時刻の入力画像（時間系のプラグイン）、描画スケール（プレビュー解像度）

対応していないもの（これらを必須とするプラグインは読み込み時か描画時に失敗し、元の画像のまま続ける）:

- オーバーレイ（画面上の操作 UI）、カスタムパラメータ、パラメトリックパラメータ
- 複数入力（マスク入力・トランジション）、ジェネレーター、タイル描画、OpenGL 描画
- プラグイン側からのパラメータ変更の通知（`InstanceChanged`）、クリップ設定の問い合わせ（`GetClipPreferences` 等）

**動作を確認したのは OpenFX 公式リポジトリのサンプル 5 種だけ**（Basic / Invert / ChoiceParams と、
C++ サポートライブラリ版の Invert / Retimer）。市販のプラグインでは試していない。
動かないプラグインを調べるときは、環境変数 `ZND_OFX_TRACE=1` で、ホストが呼んだアクションと
プラグインが読もうとして見つからなかったプロパティが stderr に出る。
