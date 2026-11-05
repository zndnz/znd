/* サンプルプラグイン（ABI 3）: 画像を広げて縁を付けるエフェクトと、音量を変える音声エフェクトと、
 * 下のレイヤーまでの絵を反転して透かすエフェクト。
 *
 *   cc -O2 -shared -fPIC -I../../sdk -o outline.dylib outline.c -lm
 *
 * ABI 3 のホスト関数（出力の大きさの変更 set_output・組み込みエフェクトの呼び出し apply_effect）と、
 * 音声エフェクト（znd_plugin_process_audio）、下のレイヤーまでの絵（backdrop）の使い方を示す。
 */
#include "znd_plugin.h"
#include <math.h>
#include <stddef.h>
#include <stdio.h>

static const ZndParamDesc OUTLINE_PARAMS[] = {
    {"width", "幅（px）", ZND_PARAM_FLOAT, ZND_PARAM_PIXELS, {4, 0, 0, 0}, 0.0, 500.0, 0},
    {"color", "色", ZND_PARAM_COLOR, 0, {1, 1, 1, 1}, 0, 0, 0},
    {"soft", "ぼかし（px）", ZND_PARAM_FLOAT, 0, {0, 0, 0, 0}, 0.0, 100.0, 0},
};

static const ZndParamDesc GAIN_PARAMS[] = {
    {"gain", "倍率", ZND_PARAM_FLOAT, 0, {0.5, 0, 0, 0}, 0.0, 4.0, 0},
};

static const ZndEffectDesc EFFECTS[] = {
    {"org.znd.example.outline", "縁取り（ABI 3 サンプル）", "サンプル", ZND_EFFECT_THREAD_SAFE, 3, OUTLINE_PARAMS},
    {"org.znd.example.gain", "音量（ABI 3 サンプル）", "サンプル", ZND_EFFECT_THREAD_SAFE | ZND_EFFECT_AUDIO, 1, GAIN_PARAMS},
    {"org.znd.example.see_through", "反転して透かす（ABI 3 サンプル）", "サンプル", ZND_EFFECT_THREAD_SAFE | ZND_EFFECT_READS_BELOW, 0, NULL},
};

static const ZndPluginDesc DESC = {ZND_PLUGIN_ABI, "znd サンプル", "1.0", "MIT OR Apache-2.0", 3, EFFECTS};

const ZndPluginDesc *znd_plugin_describe(void) { return &DESC; }

/* 下のレイヤーまでの絵の色を反転して、このレイヤーの形の中に描く */
static int32_t see_through(const ZndRenderArgs *a) {
  ZndImage below;
  if (a->host->backdrop(a->host->ctx, &below) != 0) return 1;
  if (below.width != a->width || below.height != a->height) return 1;
  for (size_t i = 0; i < (size_t)a->width * a->height; i++) {
    float *o = a->rgba + i * 4;
    const float *b = below.rgba + i * 4;
    float m = o[3];
    for (int k = 0; k < 3; k++) o[k] = (b[3] - b[k]) * m;
    o[3] = b[3] * m;
  }
  return 0;
}

int32_t znd_plugin_render(uint32_t effect, const ZndRenderArgs *a) {
  if (!a->host) return 1;
  if (effect == 2) return see_through(a);
  if (effect != 0) return 1;
  const double *v = a->values;
  int pad = (int)ceil(v[0]);
  if (pad <= 0) return 0;
  uint32_t W = a->width + 2 * pad, H = a->height + 2 * pad;
  ZndImage out;
  if (a->host->set_output(a->host->ctx, W, H, -pad, -pad, &out) != 0) return 1;
  /* 縁: 入力のアルファを半径 pad で膨らませたもの */
  for (uint32_t y = 0; y < H; y++) {
    for (uint32_t x = 0; x < W; x++) {
      float m = 0.0f;
      for (int dy = -pad; dy <= pad && m < 1.0f; dy++) {
        int sy = (int)y - pad + dy;
        if (sy < 0 || sy >= (int)a->height) continue;
        for (int dx = -pad; dx <= pad; dx++) {
          int sx = (int)x - pad + dx;
          if (sx < 0 || sx >= (int)a->width || dx * dx + dy * dy > pad * pad) continue;
          float al = a->rgba[((size_t)sy * a->width + sx) * 4 + 3];
          if (al > m) m = al;
        }
      }
      float *o = out.rgba + ((size_t)y * W + x) * 4;
      for (int k = 0; k < 3; k++) o[k] = (float)v[4 + k] * m;
      o[3] = m;
    }
  }
  if (v[8] > 0) {
    char json[64];
    snprintf(json, sizeof json, "{\"radius\": %g}", v[8]);
    a->host->apply_effect(a->host->ctx, "gaussian_blur", json, &out);
  }
  /* 縁の上に元の画像を重ねる（プリマルチプライドの over） */
  for (uint32_t y = 0; y < a->height; y++) {
    for (uint32_t x = 0; x < a->width; x++) {
      const float *s = a->rgba + ((size_t)y * a->width + x) * 4;
      float *o = out.rgba + ((size_t)(y + pad) * W + x + pad) * 4;
      for (int k = 0; k < 4; k++) o[k] = s[k] + o[k] * (1.0f - s[3]);
    }
  }
  return 0;
}

int32_t znd_plugin_process_audio(uint32_t effect, const ZndAudioArgs *a) {
  if (effect != 1) return 1;
  float g = (float)a->values[0];
  for (uint32_t i = 0; i < a->frames * 2; i++) a->samples[i] *= g;
  return 0;
}
