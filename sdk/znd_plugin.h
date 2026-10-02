/*
 * znd プラグイン API（エフェクト）  ABI バージョン 2
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

#define ZND_PLUGIN_ABI 2

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
  ZND_PARAM_POINT = 4   /* values[0..2] = バッファ画素座標（左上原点） */
};

/* パラメータのフラグ */
#define ZND_PARAM_PIXELS 1u /* FLOAT: 単位が px。プレビュー解像度に合わせてホストが縮尺を掛けて渡す */

/* エフェクトのフラグ */
#define ZND_EFFECT_TIME_DEPENDENT 1u /* パラメータが同じでも時刻で結果が変わる（ノイズ等） */
#define ZND_EFFECT_THREAD_SAFE 2u    /* render を複数スレッドから同時に呼んでよい */

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

typedef struct ZndRenderArgs {
  float *rgba;           /* width*height*4。リニアライト・プリマルチプライド。その場で書き換える */
  uint32_t width, height;
  double time;           /* コンポの時刻（秒） */
  double fps;            /* コンポのフレームレート */
  double scale;          /* バッファ画素 / レイヤー座標 1 単位（1/2 解像度のプレビューなら 0.5） */
  uint32_t seed;         /* レイヤーごとに異なる乱数の種 */
  uint32_t param_count;
  const double *values;  /* param_count * 4 個 */
} ZndRenderArgs;

/* プラグインの定義を返す。返したポインタはライブラリが読み込まれている間ずっと有効であること。 */
ZND_EXPORT const ZndPluginDesc *znd_plugin_describe(void);

/* effect 番目のエフェクトを描く。成功は 0。0 以外を返すとホストは元の画像のまま続ける。 */
ZND_EXPORT int32_t znd_plugin_render(uint32_t effect, const ZndRenderArgs *args);

#ifdef __cplusplus
}
#endif
#endif
