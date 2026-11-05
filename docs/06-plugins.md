# 6. プラグイン

> **プラグインの導入・使用は自己責任です。** znd は読み込む前に安全性スキャン（後述）を行いますが、
> これはファイルを実行せずに中身を調べる簡易的な検査で、悪質なプラグインをすべて見抜けるわけではありません。
> 「安全」と出ても安全を保証するものではありません。入手元が信頼できないものは入れないでください。

プラグインでできること:

| 種類 | 提供できるもの |
|---|---|
| エフェクト（ネイティブ / OpenFX） | エフェクトメニューに並ぶ画像処理 |
| パッケージ（`plugin.json` のあるフォルダ） | 上記のエフェクトに加えて、メニューのコマンド・レイヤーの右クリックメニュー・ツールバーのボタン・キーボードショートカット・パネル（UI）・出力フィルター（レンダリングへの介入）・エクスプレッション用ライブラリ |

## エフェクト

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
パラメータは Float / Bool / Choice / Color / Point（ABI 3 では Text / Layer も）で、ホストがキーフレームと
エクスプレッションを評価した「その時刻の値」を渡す。px 単位の値と位置は、プレビュー解像度に
合わせてホストが縮尺を掛けてから渡すので、プラグイン側で解像度を気にする必要はない。

サンプル:

- [examples/plugins/vignette.c](../examples/plugins/vignette.c)（C。周辺減光と色かぶり）
- [examples/plugins/scanline.rs](../examples/plugins/scanline.rs)（Rust。時刻で変わるエフェクト）

```bash
cc -O2 -shared -fPIC -Isdk -o vignette.dylib examples/plugins/vignette.c
```

- [examples/plugins/outline.c](../examples/plugins/outline.c)（C。ABI 3: 画像を広げる縁取りと、音声エフェクト）

### ABI 3 で増えたもの

ABI 2 のプラグインもそのまま読み込める。ABI 3 を宣言したプラグインには、`ZndRenderArgs` の後半と
ホスト関数表 `ZndHost` が渡る。

| 項目 | 内容 |
|---|---|
| `layer_time` / `layer_duration` | レイヤーのイン点からの経過秒と、レイヤーの長さ |
| `ZND_PARAM_TEXT` | 文字列のパラメータ（`strings[i]`） |
| `ZND_PARAM_LAYER` | 他のレイヤーの参照。`host->layer_image` / `host->audio` で絵と音を取得する |
| `host->frame_at` | このレイヤーの（エフェクト前の）絵を別の時刻で取得する（残像・時間系） |
| `host->apply_effect` | 組み込みエフェクト（`gaussian_blur` など）を種類名と JSON のパラメータで呼ぶ |
| `host->set_output` | 出力の大きさと左上の位置を変える（縁取り・影・余白の除去など）。レイヤーの置き場所はホストが合わせる |
| `host->mask_count` / `host->mask_points` | レイヤーのマスクのパスを折れ線（バッファ画素座標）で受け取る（パスに沿って描くエフェクト用） |
| `host->param_at` | パラメータを別の時刻（イン点からの秒）で評価する。キーフレームで描いた曲線を、トーンカーブなどの表として読む用 |
| `host->comp_size` | レイヤーが入っているコンポの大きさ（コンポの画素。`scale` は掛けない）。画面の大きさに合わせるエフェクト用 |
| `host->backdrop` | 下のレイヤーまでの合成結果を、このバッファの画素に写して取得する。`ZND_EFFECT_READS_BELOW` を付けたエフェクトだけが読める（そのレイヤーのあるコンポは CPU で合成する）。ガラス・屈折など背景を透かす効果用 |
| `ZND_EFFECT_AUDIO` + `znd_plugin_process_audio` | 音声エフェクト。ミキサーがレイヤーの音に掛ける。音の無いレイヤー（平面など）に掛けると無音から始まるので、シンセやメトロノームも作れる。`length` はレイヤーの長さ（サンプル数）で、全体を測ってから掛ける処理に使う |

大きさを変えたエフェクトは、平面・フッテージ・プリコンポのレイヤーでは画像が広がったまま合成される。
シェイプ・テキストなどコンポ全体のバッファに描くレイヤーと調整レイヤー・出力フィルターでは、
元の大きさのバッファに貼り直される（はみ出した部分は切れる）。

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

## パッケージ（コマンド・パネル・出力フィルター・エクスプレッション）

`plugin.json` を持つフォルダをプラグインフォルダに置くと、パッケージとして読み込まれる。
サンプル: [examples/plugins/toolkit.zndplugin](../examples/plugins/toolkit.zndplugin)

```json
{
  "id": "com.example.toolkit",
  "name": "ツールキット",
  "version": "1.0.0",
  "vendor": "作者",
  "license": "MIT",
  "permissions": [],
  "effects": ["lib/fx.dylib"],
  "commands": [
    { "id": "center", "label": "中央へ", "script": "commands/layout.js", "function": "centerSelected",
      "menu": ["plugins", "layer", "toolbar"], "shortcut": "Cmd+Shift+H" }
  ],
  "panels": [{ "id": "motion", "title": "揺れと弾み", "script": "panels/motion.js", "width": 300 }],
  "output_filters": [
    { "id": "finish", "label": "仕上げ", "stage": "both", "enabled": false,
      "effects": [{ "kind": "film_grain", "params": { "amount": 8 } }] }
  ],
  "expressions": ["expressions/motion.js"]
}
```

| 項目 | 内容 |
|---|---|
| `commands` | `menu` に `plugins`（メニューバーの「プラグイン」）/ `layer`（タイムラインのレイヤーの右クリック）/ `toolbar` を並べる。`shortcut` は `Cmd` `Ctrl` `Alt` `Shift` とキー名を `+` でつなぐ。組み込みのショートカットが優先される |
| `panels` | スクリプトで中身を組み立てる浮動パネル。「プラグイン」メニューで開閉する |
| `output_filters` | コンポの最終画像（背景を敷いた後）に掛けるエフェクトの列。`stage` は `preview` / `export` / `both`。プラグインのエフェクト（`plugin:` / `ofx:`）も指定できる。「プラグイン」メニューで切り替える |
| `expressions` | ここで定義した関数は、すべてのエクスプレッションから呼べる（関数の中から `thisComp`・`thisLayer`・`time` も参照できる） |
| `exporters` | 書き出し形式（FFmpeg の出力引数の組）。書き出し画面の形式に並び、CLI では `--format plugin:<パッケージID/ID>` で使える（後述） |
| `permissions` | 必要とする権限の宣言（`network` / `process` / `filesystem`）。安全性スキャンで中身と突き合わせ、宣言なしに通信していれば危険度を上げる。`filesystem` を宣言したスクリプトは、利用者が選んだファイルを読み書きできる |
| `effects` | エフェクトのライブラリ。拡張子を省く（`lib/fx`）と、OS に合わせて `fx.dylib` / `libfx.dylib` / `.so` / `.dll` を探す |

パス はパッケージ内の相対パスのみ（`..` や絶対パスは拒否）。スクリプトはスキャンした時点の中身をメモリに持つので、
読み込み後にファイルを書き換えても次の起動まで反映されない。

### スクリプトから使えるもの

コマンドもパネルも、自動化スクリプト（`znd script`）と同じ環境で動く。

- `project`: プロジェクト全体（ファイル形式と同じ JSON）。書き換えると 1 回の取り消し単位として取り込まれる。
  実行後に構造を検査し、参照切れなどがあれば取り込まない
- `ctx`: `comp`（開いているコンポの ID）、`layer` / `layers`（選択中のレイヤー）、`frame`、`time`（秒）、`fps`、`plugin`、`projectPath`（保存先。未保存なら null）
- `host`: 画面への操作。`select(id)`、`setFrame(n)`、`setWorkArea(開始, 終了)`（フレーム。終了は含まない。引数なしで解除）、`setTime(秒)`、`status(文)`、`alert(文)`、`copyText(文)`（クリップボードへ）、`openProjectFolder()`（プロジェクトのフォルダを開く）、`openComp(id)`、
  `openPanel(key)` / `closePanel(key)`、`setFilter(key, on)`、`runCommand(key)`（key は `パッケージID/ID`）
- `log(...)`（「プラグイン」ウィンドウに出る）と補助関数 `comp()` `layer()` `key()` `setKeys()` `flicks()` `seconds()` `newId()`、
  `addEffect(レイヤー, 種類, 名前)`（既定値のエフェクトを足して返す）・`setParam(エフェクト, パラメータ, 値)`・`findEffect(レイヤー, 名前)`・`addSolid(コンポ, 名前, 色)`（コンポの大きさの平面を一番上に足す）・`addText(コンポ, 名前, 文字, サイズ)`（コンポの中心にテキストレイヤーを一番上に足す）

- `host.audioLevels(layerIds, callback, { comp, data })`: レイヤーの音の大きさを 1 フレームごとに測り（音量・エフェクトを掛けた後の RMS の dBFS。無音は -120）、`callback({ レイヤーID: [dB, ...] }, data)` を呼ぶ。パネルから呼ぶと、callback の戻り値がパネルの状態になる
- `filesystem` 権限があるときだけ: `host.openFile({ extensions, binary, data }, callback)`（利用者が選んだファイルを読み、
  同じスクリプトの `callback(中身, パス, data)` を呼ぶ。パネルから呼んだときは、コールバックが返したオブジェクトがパネルの新しい状態になる。テキストは UTF-8 / UTF-16 / Shift_JIS を見分ける）、
  `host.saveFile(名前, 中身, { extensions })`（保存先を選んでもらって書く。中身はバイト列の配列も可）、
  `host.importFiles({ extensions })`（素材を選んでもらい、再生ヘッドの位置に読み込む）

スクリプトの環境にはネットワーク・別プログラムへの手段が無く、ファイルは利用者がダイアログで選んだものにしか触れない。実行は 10 秒で打ち切る。

### 書き出し形式（exporters）

```json
"exporters": [{
  "id": "x264", "label": "x264（品質指定）", "extension": "mp4", "even": true,
  "video_args": ["-c:v", "libx264", "-preset", "{preset}", "-crf", "{crf}", "-pix_fmt", "yuv420p"],
  "audio_args": ["-c:a", "aac", "-b:a", "{abr}k"],
  "params": [
    { "id": "crf", "label": "CRF", "type": "number", "default": 18, "min": 0, "max": 51 },
    { "id": "preset", "label": "プリセット", "type": "choice", "default": 1, "options": ["fast", "medium", "slow"] },
    { "id": "abr", "label": "音声 kbps", "default": 192 }
  ]
}]
```

| 項目 | 内容 |
|---|---|
| `video_args` / `audio_args` | 入力（生フレームのパイプと音声の WAV）の後ろ・出力ファイルの前に置く引数。`{パラメータID}` は値に置き換わり、空になった引数は消える。`-i` は使えない |
| `alpha` | アルファ付き（16bit RGBA）でパイプに流す。透過の書き出しができる |
| `even` | 偶数の解像度が必要（4:2:0 など） |
| `audio_only` | 音声だけを書き出す（映像はレンダリングしない） |
| `single_frame` | 1 フレームだけ書き出す（アプリでは今のフレーム、CLI では `--range` の先頭） |
| `params` | 書き出し画面に並ぶ設定。`type` は `number` / `choice`（`options` は文字列か `{label, value}`）/ `bool`（0/1）/ `text` / `args`（空白で区切った複数の引数。引数そのものが `{ID}` のとき展開する） |

### パネル

`panel(state)` が部品の配列を返し、部品が操作されると `onEvent(id, value, state)` が呼ばれる。
`state` はパネルごとにホストが持ち回る（初回は `{}`）。プロジェクトの書き換えは `onEvent` でのみ受け付ける。
部品は `{ type, id, label, value, ... }`:

| type | 追加の項目 | value |
|---|---|---|
| `heading` / `label` / `hint` | `text` | — |
| `separator` | — | — |
| `button` | — | `null` |
| `slider` | `min` `max` `step` | 数値 |
| `number` | `speed` | 数値 |
| `checkbox` | — | 真偽値 |
| `text` | — | 文字列（確定時に送る） |
| `choice` | `options`（文字列の配列） | 番号 |
| `color` | — | `[r, g, b, a]`（0〜1） |
| `row` | `items`（部品の配列） | 横に並べる |

## 安全性スキャンと危険度

すべてのプラグイン（単体のライブラリ・OpenFX バンドル・パッケージ）は、読み込む前に中身を調べて危険度を付ける。
特に、外部のサーバーを経由する悪質なもの（情報を外へ送る、外からプログラムを取ってきて実行する、外から命令を受ける）を、
通信機能と他の痕跡の組み合わせで見分ける。

| 見る項目 | 例 |
|---|---|
| 通信 | `socket` `connect` `getaddrinfo`、libcurl、NSURLSession、WinHTTP、スクリプトの `fetch` |
| 通信先 | URL・生の IP アドレス。使い捨てのファイル置き場・チャットの Webhook・トンネル・短縮 URL は特に重く見る |
| プログラムの実行 | `system` `popen` `execve` `posix_spawn`、`/bin/sh`、`curl ... \| sh` |
| 個人情報 | `~/.ssh`、キーチェーン、ブラウザのパスワード・Cookie、暗号資産のウォレット |
| 監視 | キー入力の横取り、画面の取り込み、クリップボード |
| 常駐化 | LaunchAgents、crontab、Run レジストリキー、シェルの起動ファイル |
| 解析妨害・難読化 | `PT_DENY_ATTACH`、長い base64、エスケープだらけのスクリプト |
| 宣言との食い違い | 通信するのに `permissions` に `network` が無い、宣言されていない実行ファイルの同梱 |

組み合わせ（通信＋個人情報 → 持ち出しの疑い、通信＋実行 → 外部からのプログラム取得の疑い、通信＋常駐化 → 遠隔操作の疑い）は
加点が大きく、持ち出し・遠隔操作の疑いは点数に関係なく「重大」になる。

| 危険度 | 点数 | 扱い |
|---|---|---|
| 安全 | 0 | 読み込む |
| 低 | 1〜24 | 読み込む |
| 中 | 25〜49 | 読み込む（一覧に警告を出す） |
| 高 | 50〜79 | **読み込まない**。利用者が内容を確認し、自己責任で許可したときだけ読み込む |
| 重大 | 80〜 | 同上 |

許可は「プラグイン」ウィンドウの「自己責任で許可して読み込む…」で行い、中身の SHA-256 に結び付けて
`~/Library/Application Support/znd/plugin-trust.json`（Linux は `~/.config/znd/`、Windows は `%APPDATA%\znd\`）に記録する。
ファイルが差し替えられると許可は無効になり、再び止まる。

CLI では読み込まずにスキャンだけできる:

```bash
cargo run -p znd-cli -- plugin-scan path/to/plugin.dylib
```

**限界**: 実行時に組み立てる文字列・暗号化や圧縮で隠したコード・正規のライブラリに見せかけた処理は見抜けない。
逆に、ライセンス認証や更新確認のために通信する正当なプラグインも「通信」として検出される。結果は判断材料であり、最終的な判断と責任は利用者にある。
