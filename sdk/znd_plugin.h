/*
 * znd プラグイン API（エフェクト）  ABI バージョン 3（2 のプラグインもそのまま読み込める）
 *
 * 動的ライブラリ（.dylib / .so / .dll）1 つが、エフェクトを 1 つ以上提供する。
 * ホストは znd_plugin_describe() で定義を読み、エフェクトメニューとパラメータ欄を作り、
 * フレームごとに znd_plugin_render() を呼ぶ。パラメータはホスト側でキーフレーム・
 * エクスプレッションを評価した「その時刻の値」が渡される。
 *
 * 置き場所（どれか）:
 *   - 環境変数 ZND_PLUGIN_PATH に並べたフォルダ（: 区切り。Windows は ;）
 *   - macOS: ~/Library/Application Support/znd/plugins
 *     Linux: ~/.local/share/znd/plugins      Windows: %APPDATA%\znd\plugins
 *
 * 注意: プラグインはホストと同じプロセス・同じ権限で動く。信頼できるものだけを入れること。
 *
 * このヘッダーは MIT OR Apache-2.0。プラグイン自体のライセンスは作者が自由に決められる。
 */
#ifndef ZND_PLUGIN_H
#define ZND_PLUGIN_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define ZND_PLUGIN_ABI 3

#if defined(_WIN32)
#define ZND_EXPORT __declspec(dllexport)
#else
#define ZND_EXPORT __attribute__((visibility("default")))
#endif

/* パラメータの種類。values には 1 パラメータあたり double 4 個の枠がある。 */
enum {
  ZND_PARAM_FLOAT = 0,  /* values[0] */
  ZND_PARAM_BOOL = 1,   /* values[0] = 0 または 1 */
  ZND_PARAM_CHOICE = 2, /* values[0] = 選択肢の番号（0 始まり） */
  ZND_PARAM_COLOR = 3,  /* values[0..4] = リニアライトの RGBA（ストレートアルファ） */
  ZND_PARAM_POINT = 4,  /* values[0..2] = バッファ画素座標（左上原点） */
  /* ---- ABI 3 ---- */
  ZND_PARAM_TEXT = 5,   /* strings[i] = UTF-8 の文字列（values は未使用） */
  ZND_PARAM_LAYER = 6   /* values[0] = レイヤーが選ばれていれば 1。画像・音声は host から取得する */
};

/* パラメータのフラグ */
#define ZND_PARAM_PIXELS 1u /* FLOAT: 単位が px。プレビュー解像度に合わせてホストが縮尺を掛けて渡す */

/* エフェクトのフラグ */
#define ZND_EFFECT_TIME_DEPENDENT 1u /* パラメータが同じでも時刻で結果が変わる（ノイズ等） */
#define ZND_EFFECT_THREAD_SAFE 2u    /* render を複数スレッドから同時に呼んでよい */
#define ZND_EFFECT_AUDIO 4u          /* ABI 3: 音声エフェクト。znd_plugin_process_audio が呼ばれ、映像には何もしない */

typedef struct ZndParamDesc {
  const char *id;      /* プロジェクトに保存される名前。英数字と _ 。後から変えないこと */
  const char *label;   /* 画面に出す名前（UTF-8） */
  uint32_t type;       /* ZND_PARAM_* */
  uint32_t flags;      /* ZND_PARAM_PIXELS など */
  double def[4];       /* 既定値。COLOR は sRGB 表示値の RGBA、POINT はレイヤーの幅・高さに対する割合 */
  double min, max;     /* FLOAT の範囲 */
  const char *choices; /* CHOICE の選択肢を '|' で区切ったもの。それ以外は NULL */
} ZndParamDesc;

typedef struct ZndEffectDesc {
  const char *id;       /* 一意な名前（例: "com.example.vignette"）。後から変えないこと */
  const char *name;     /* メニューに出す名前（UTF-8） */
  const char *category; /* メニューの分類。NULL なら「プラグイン」 */
  uint32_t flags;       /* ZND_EFFECT_* */
  uint32_t param_count;
  const ZndParamDesc *params;
} ZndEffectDesc;

typedef struct ZndPluginDesc {
  uint32_t abi_version; /* ZND_PLUGIN_ABI */
  const char *vendor;   /* 作者・配布元 */
  const char *version;
  const char *license;  /* 利用条件（About とクレジットの確認用に表示される） */
  uint32_t effect_count;
  const ZndEffectDesc *effects;
} ZndPluginDesc;

/* ABI 3: ホストが確保した画像。rgba は width*height*4（リニアライト・プリマルチプライド）。
 * ホストから受け取ったものは znd_plugin_render から戻るまで有効。 */
typedef struct ZndImage {
  float *rgba;
  uint32_t width, height;
} ZndImage;

/* ABI 3: render の中から呼べるホストの関数。どれも成功は 0。ctx には ZndHost.ctx を渡す。 */
typedef struct ZndHost {
  void *ctx;
  /* このレイヤーの（エフェクトを掛ける前の）絵を、コンポ時刻 time（秒）で取得する。残像・時間系のエフェクト用 */
  int32_t (*frame_at)(void *ctx, double time, ZndImage *out);
  /* param 番目（LAYER パラメータ）で選ばれたレイヤーの絵を、このバッファと同じ大きさで取得する */
  int32_t (*layer_image)(void *ctx, uint32_t param, ZndImage *out);
  /* param 番目（LAYER パラメータ）で選ばれたレイヤーの音声（モノラル 48kHz）を、現在時刻を中心に len サンプル取得する */
  int32_t (*audio)(void *ctx, uint32_t param, float *out, uint32_t len);
  /* 組み込みエフェクトを img に掛ける。kind は種類名（"gaussian_blur" など）、params_json はパラメータの JSON
   * （例: {"radius": 8}。px 単位の値はレイヤー座標で書く）。大きさが変わるエフェクトでは img が差し替わる */
  int32_t (*apply_effect)(void *ctx, const char *kind, const char *params_json, ZndImage *img);
  /* 出力の大きさを変える。width×height の透明な画像を作って out に返す。プラグインはそこへ結果を書く
   * （入力の args->rgba は読み続けてよい）。(offset_x, offset_y) は出力の左上の位置を入力の画素座標で表したもの。
   * 例: 四方に 10px 広げるなら (w+20, h+20, -10, -10)。呼んだ場合、args->rgba への書き込みは捨てられる */
  int32_t (*set_output)(void *ctx, uint32_t width, uint32_t height, int32_t offset_x, int32_t offset_y, ZndImage *out);
} ZndHost;

typedef struct ZndRenderArgs {
  float *rgba;           /* width*height*4。リニアライト・プリマルチプライド。その場で書き換える */
  uint32_t width, height;
  double time;           /* コンポの時刻（秒） */
  double fps;            /* コンポのフレームレート */
  double scale;          /* バッファ画素 / レイヤー座標 1 単位（1/2 解像度のプレビューなら 0.5） */
  uint32_t seed;         /* レイヤーごとに異なる乱数の種 */
  uint32_t param_count;
  const double *values;  /* param_count * 4 個 */
  /* ---- ABI 3（ABI 2 のプラグインには渡らない） ---- */
  const char *const *strings; /* param_count 個。TEXT パラメータだけ非 NULL */
  double layer_time;          /* レイヤーのイン点からの経過秒 */
  double layer_duration;      /* レイヤーの長さ（秒） */
  const ZndHost *host;
} ZndRenderArgs;

/* ABI 3: 音声エフェクトに渡す 1 ブロック（48kHz・ステレオ）。ブロックは数百サンプルずつ、時刻順に渡されるとは限らない。
 * 状態を持つ処理（フィルター・残響）は instance ごとに状態を持ち、start が前回の続きでなければ状態を捨てること。 */
typedef struct ZndAudioArgs {
  float *samples;       /* frames*2（L, R の交互）。その場で書き換える */
  uint32_t frames;
  uint32_t sample_rate; /* 48000 */
  int64_t start;        /* samples[0] の、レイヤーのイン点から数えたサンプル位置 */
  double time;          /* samples[0] のコンポ時刻（秒） */
  uint64_t instance;    /* エフェクトの実体ごとに異なる値 */
  uint32_t seed;
  uint32_t param_count;
  const double *values; /* param_count * 4 個（ブロックの先頭時刻で評価した値。px の縮尺は掛からない） */
  const char *const *strings;
} ZndAudioArgs;

/* プラグインの定義を返す。返したポインタはライブラリが読み込まれている間ずっと有効であること。 */
ZND_EXPORT const ZndPluginDesc *znd_plugin_describe(void);

/* effect 番目のエフェクトを描く。成功は 0。0 以外を返すとホストは元の画像のまま続ける。 */
ZND_EXPORT int32_t znd_plugin_render(uint32_t effect, const ZndRenderArgs *args);

/* ABI 3（任意）: ZND_EFFECT_AUDIO の effect 番目の音声エフェクトを処理する。成功は 0。
 * 0 以外を返すと、ホストはそのブロックを元の音のまま続ける。音の無いレイヤー（平面など）に掛けると、無音から始まる */
ZND_EXPORT int32_t znd_plugin_process_audio(uint32_t effect, const ZndAudioArgs *args);

#ifdef __cplusplus
}
#endif
#endif
