/* サンプルプラグイン: 周辺減光と、色かぶり。
 *
 *   cc -O2 -shared -fPIC -I../../sdk -o vignette.dylib vignette.c     (macOS)
 *   cc -O2 -shared -fPIC -I../../sdk -o vignette.so vignette.c        (Linux)
 *
 * できたファイルをプラグインフォルダに置くと、エフェクトメニューの「サンプル」に現れる。
 */
#include "znd_plugin.h"
#include <math.h>
#include <stddef.h>

static const ZndParamDesc VIGNETTE_PARAMS[] = {
    {"amount", "強さ", ZND_PARAM_FLOAT, 0, {0.6, 0, 0, 0}, 0.0, 1.0, 0},
    {"radius", "半径（px）", ZND_PARAM_FLOAT, ZND_PARAM_PIXELS, {400, 0, 0, 0}, 1.0, 10000.0, 0},
    {"center", "中心", ZND_PARAM_POINT, 0, {0.5, 0.5, 0, 0}, 0, 0, 0},
    {"shape", "形", ZND_PARAM_CHOICE, 0, {0, 0, 0, 0}, 0, 0, "円|ひし形"},
    {"invert", "反転", ZND_PARAM_BOOL, 0, {0, 0, 0, 0}, 0, 0, 0},
};

static const ZndParamDesc TINT_PARAMS[] = {
    {"color", "色", ZND_PARAM_COLOR, 0, {1.0, 0.5, 0.0, 1.0}, 0, 0, 0},
    {"mix", "混ぜる量", ZND_PARAM_FLOAT, 0, {0.5, 0, 0, 0}, 0.0, 1.0, 0},
};

static const ZndEffectDesc EFFECTS[] = {
    {"org.znd.example.vignette", "周辺減光", "サンプル", ZND_EFFECT_THREAD_SAFE, 5, VIGNETTE_PARAMS},
    {"org.znd.example.tint", "色かぶり", "サンプル", ZND_EFFECT_THREAD_SAFE, 2, TINT_PARAMS},
};

static const ZndPluginDesc DESC = {ZND_PLUGIN_ABI, "znd サンプル", "1.0", "MIT OR Apache-2.0", 2, EFFECTS};

const ZndPluginDesc *znd_plugin_describe(void) { return &DESC; }

int32_t znd_plugin_render(uint32_t effect, const ZndRenderArgs *a) {
  const double *v = a->values;
  if (effect == 0) {
    double amount = v[0], radius = v[4] > 1e-6 ? v[4] : 1e-6;
    double cx = v[8], cy = v[9];
    int diamond = (int)v[12] == 1, invert = v[16] != 0.0;
    for (uint32_t y = 0; y < a->height; y++) {
      for (uint32_t x = 0; x < a->width; x++) {
        double dx = fabs(x + 0.5 - cx) / radius, dy = fabs(y + 0.5 - cy) / radius;
        double d = diamond ? dx + dy : sqrt(dx * dx + dy * dy);
        double k = d < 1.0 ? d * d : 1.0; /* 中心 0 → 縁 1 */
        if (invert) k = 1.0 - k;
        float gain = (float)(1.0 - amount * k);
        float *p = a->rgba + ((size_t)y * a->width + x) * 4;
        p[0] *= gain; /* プリマルチプライドなので、色だけに掛ければ「暗くする」になる */
        p[1] *= gain;
        p[2] *= gain;
      }
    }
    return 0;
  }
  if (effect == 1) {
    float mix = (float)v[4];
    size_t n = (size_t)a->width * a->height;
    for (size_t i = 0; i < n; i++) {
      float *p = a->rgba + i * 4;
      float alpha = p[3];
      for (int k = 0; k < 3; k++) p[k] += ((float)v[k] * alpha - p[k]) * mix;
    }
    return 0;
  }
  return 1;
}
