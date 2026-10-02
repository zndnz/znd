/* ホストの検証用 OpenFX プラグイン。公式ヘッダー（include/）でコンパイルする。
 *
 *  0: org.znd.test.gain   各種パラメータ・メモリ／マルチスレッド／メッセージスイート・座標の向き
 *  1: org.znd.test.echo   別時刻の入力と、別時刻のパラメータ値
 *  2: org.znd.test.bytes  8bit しか扱えないプラグイン（ホスト側の形式変換）
 *  3: org.znd.test.gen    ジェネレーター専用（ホストが理由を示して断ること）
 */
#include "ofxImageEffect.h"
#include "ofxMemory.h"
#include "ofxMessage.h"
#include "ofxMultiThread.h"
#include <string.h>

static OfxHost *gHost;
static const OfxPropertySuiteV1 *gProp;
static const OfxImageEffectSuiteV1 *gFx;
static const OfxParameterSuiteV1 *gParam;
static const OfxMemorySuiteV1 *gMem;
static const OfxMultiThreadSuiteV1 *gThread;
static const OfxMessageSuiteV1 *gMsg;

static OfxStatus fetch_suites(void) {
  if (!gHost) return kOfxStatErrMissingHostFeature;
  gProp = (const OfxPropertySuiteV1 *)gHost->fetchSuite(gHost->host, kOfxPropertySuite, 1);
  gFx = (const OfxImageEffectSuiteV1 *)gHost->fetchSuite(gHost->host, kOfxImageEffectSuite, 1);
  gParam = (const OfxParameterSuiteV1 *)gHost->fetchSuite(gHost->host, kOfxParameterSuite, 1);
  gMem = (const OfxMemorySuiteV1 *)gHost->fetchSuite(gHost->host, kOfxMemorySuite, 1);
  gThread = (const OfxMultiThreadSuiteV1 *)gHost->fetchSuite(gHost->host, kOfxMultiThreadSuite, 1);
  gMsg = (const OfxMessageSuiteV1 *)gHost->fetchSuite(gHost->host, kOfxMessageSuite, 1);
  if (!gProp || !gFx || !gParam || !gMem || !gThread || !gMsg) return kOfxStatErrMissingHostFeature;
  /* 無いスイートは NULL が返ること */
  if (gHost->fetchSuite(gHost->host, "NoSuchSuite", 1)) return kOfxStatErrFatal;
  return kOfxStatOK;
}

static OfxStatus describe_common(OfxImageEffectHandle fx, const char *label, const char *depth, const char *context) {
  OfxPropertySetHandle props;
  gFx->getPropertySet(fx, &props);
  gProp->propSetString(props, kOfxPropLabel, 0, label);
  gProp->propSetString(props, kOfxImageEffectPluginPropGrouping, 0, "znd テスト");
  gProp->propSetString(props, kOfxImageEffectPropSupportedContexts, 0, context);
  gProp->propSetString(props, kOfxImageEffectPropSupportedPixelDepths, 0, depth);
  return kOfxStatOK;
}

static void define_clips(OfxImageEffectHandle fx, int temporal) {
  OfxPropertySetHandle props;
  gFx->clipDefine(fx, kOfxImageEffectOutputClipName, &props);
  gProp->propSetString(props, kOfxImageEffectPropSupportedComponents, 0, kOfxImageComponentRGBA);
  gFx->clipDefine(fx, kOfxImageEffectSimpleSourceClipName, &props);
  gProp->propSetString(props, kOfxImageEffectPropSupportedComponents, 0, kOfxImageComponentRGBA);
  gProp->propSetInt(props, kOfxImageEffectPropTemporalClipAccess, 0, temporal);
}

/* ---------- 0: gain ---------- */

static OfxStatus gain_describe_in_context(OfxImageEffectHandle fx) {
  OfxParamSetHandle ps;
  OfxPropertySetHandle p;
  define_clips(fx, 0);
  gFx->getParamSet(fx, &ps);

  gParam->paramDefine(ps, kOfxParamTypeGroup, "grp", &p);
  gParam->paramDefine(ps, kOfxParamTypeDouble, "gain", &p);
  gProp->propSetString(p, kOfxPropLabel, 0, "ゲイン");
  gProp->propSetDouble(p, kOfxParamPropDefault, 0, 1.0);
  gProp->propSetDouble(p, kOfxParamPropMin, 0, 0.0);
  gProp->propSetDouble(p, kOfxParamPropDisplayMin, 0, 0.0);
  gProp->propSetDouble(p, kOfxParamPropDisplayMax, 0, 4.0);
  gProp->propSetString(p, kOfxParamPropParent, 0, "grp");

  gParam->paramDefine(ps, kOfxParamTypeRGBA, "tint", &p);
  {
    double white[4] = {1, 1, 1, 1};
    gProp->propSetDoubleN(p, kOfxParamPropDefault, 4, white);
  }
  gParam->paramDefine(ps, kOfxParamTypeBoolean, "invert", &p);
  gProp->propSetInt(p, kOfxParamPropDefault, 0, 0);

  gParam->paramDefine(ps, kOfxParamTypeChoice, "channel", &p);
  gProp->propSetString(p, kOfxParamPropChoiceOption, 0, "すべて");
  gProp->propSetString(p, kOfxParamPropChoiceOption, 1, "赤だけ");
  gProp->propSetInt(p, kOfxParamPropDefault, 0, 0);

  gParam->paramDefine(ps, kOfxParamTypeDouble2D, "marker", &p);
  gProp->propSetString(p, kOfxParamPropDoubleType, 0, kOfxParamDoubleTypeXYAbsolute);

  gParam->paramDefine(ps, kOfxParamTypeInteger2D, "shift", &p);
  gParam->paramDefine(ps, kOfxParamTypeString, "note", &p);
  gProp->propSetString(p, kOfxParamPropDefault, 0, "");

  /* 隠しパラメータはホストの画面に出ない（値は既定値のまま読める） */
  gParam->paramDefine(ps, kOfxParamTypeDouble, "hidden", &p);
  gProp->propSetDouble(p, kOfxParamPropDefault, 0, 1.0);
  gProp->propSetInt(p, kOfxParamPropSecret, 0, 1);

  /* 同じ名前は二重に定義できない */
  if (gParam->paramDefine(ps, kOfxParamTypeDouble, "gain", &p) != kOfxStatErrExists) return kOfxStatErrFatal;
  return kOfxStatOK;
}

typedef struct {
  const float *src;
  float *dst;
  int w, h, src_row, dst_row;
  double gain, tint[4];
  int invert, channel, shift_x;
} GainJob;

static void gain_rows(unsigned int index, unsigned int count, void *arg) {
  GainJob *j = (GainJob *)arg;
  int y0 = (int)((long)j->h * index / count), y1 = (int)((long)j->h * (index + 1) / count);
  int x, y, k;
  for (y = y0; y < y1; y++) {
    const float *s = (const float *)((const char *)j->src + (long)y * j->src_row);
    float *d = (float *)((char *)j->dst + (long)y * j->dst_row);
    for (x = 0; x < j->w; x++) {
      int sx = x - j->shift_x;
      float px[4] = {0, 0, 0, 0};
      if (sx >= 0 && sx < j->w) memcpy(px, s + sx * 4, sizeof px);
      for (k = 0; k < 3; k++) {
        if (j->channel == 1 && k != 0) continue;
        px[k] = (float)(px[k] * j->gain * j->tint[k]);
        if (j->invert) px[k] = px[3] - px[k];
      }
      memcpy(d + x * 4, px, sizeof px);
    }
  }
}

static OfxStatus image_of(OfxImageClipHandle clip, OfxTime t, OfxPropertySetHandle *img, void **data, OfxRectI *bounds, int *row) {
  OfxStatus st = gFx->clipGetImage(clip, t, NULL, img);
  if (st != kOfxStatOK) return st;
  gProp->propGetPointer(*img, kOfxImagePropData, 0, data);
  gProp->propGetIntN(*img, kOfxImagePropBounds, 4, &bounds->x1);
  gProp->propGetInt(*img, kOfxImagePropRowBytes, 0, row);
  return *data ? kOfxStatOK : kOfxStatFailed;
}

static OfxStatus gain_render(OfxImageEffectHandle fx, OfxPropertySetHandle in) {
  OfxImageClipHandle src_clip, dst_clip;
  OfxPropertySetHandle src_img, dst_img;
  OfxParamSetHandle ps;
  OfxParamHandle h;
  OfxRectI sb, db, window;
  OfxTime time;
  void *sp, *dp;
  GainJob j;
  double mx, my, hidden, scale[2];
  int sy, n, dim;
  unsigned int cpus = 1;
  char *note = NULL;
  void *scratch = NULL;

  gProp->propGetDouble(in, kOfxPropTime, 0, &time);
  gProp->propGetIntN(in, kOfxImageEffectPropRenderWindow, 4, &window.x1);
  gProp->propGetDoubleN(in, kOfxImageEffectPropRenderScale, 2, scale);
  gFx->clipGetHandle(fx, kOfxImageEffectSimpleSourceClipName, &src_clip, NULL);
  gFx->clipGetHandle(fx, kOfxImageEffectOutputClipName, &dst_clip, NULL);
  if (image_of(src_clip, time, &src_img, &sp, &sb, &j.src_row) != kOfxStatOK) return kOfxStatFailed;
  if (image_of(dst_clip, time, &dst_img, &dp, &db, &j.dst_row) != kOfxStatOK) return kOfxStatFailed;

  gFx->getParamSet(fx, &ps);
  gParam->paramGetHandle(ps, "gain", &h, NULL);
  gParam->paramGetValueAtTime(h, time, &j.gain);
  gParam->paramGetHandle(ps, "tint", &h, NULL);
  gParam->paramGetValue(h, &j.tint[0], &j.tint[1], &j.tint[2], &j.tint[3]);
  gParam->paramGetHandle(ps, "invert", &h, NULL);
  gParam->paramGetValue(h, &j.invert);
  gParam->paramGetHandle(ps, "channel", &h, NULL);
  gParam->paramGetValue(h, &j.channel);
  gParam->paramGetHandle(ps, "marker", &h, NULL);
  gParam->paramGetValue(h, &mx, &my);
  gParam->paramGetHandle(ps, "shift", &h, NULL);
  gParam->paramGetValue(h, &j.shift_x, &sy);
  gParam->paramGetHandle(ps, "note", &h, NULL);
  gParam->paramGetValue(h, &note);
  gParam->paramGetHandle(ps, "hidden", &h, NULL);
  gParam->paramGetValue(h, &hidden);
  j.gain *= hidden;
  /* 無いパラメータは失敗を返すこと */
  if (gParam->paramGetHandle(ps, "nope", &h, NULL) == kOfxStatOK) return kOfxStatErrFatal;
  /* 無いプロパティ・範囲外の添字の扱い */
  if (gProp->propGetInt(in, "NoSuchProperty", 0, &n) != kOfxStatErrUnknown) return kOfxStatErrFatal;
  if (gProp->propGetDouble(in, kOfxPropTime, 5, &mx) != kOfxStatErrBadIndex) return kOfxStatErrFatal;
  gProp->propGetDimension(in, kOfxImageEffectPropRenderWindow, &dim);
  if (dim != 4) return kOfxStatErrFatal;

  j.src = (const float *)sp;
  j.dst = (float *)dp;
  j.w = db.x2 - db.x1;
  j.h = db.y2 - db.y1;
  if (window.x2 - window.x1 != j.w || sb.x2 - sb.x1 != j.w) return kOfxStatErrFatal;

  /* メモリスイート */
  if (gMem->memoryAlloc(fx, 64, &scratch) != kOfxStatOK || !scratch) return kOfxStatErrMemory;
  memset(scratch, 0, 64);
  gMem->memoryFree(scratch);

  /* マルチスレッドスイート: 行を分担する */
  gThread->multiThreadNumCPUs(&cpus);
  if (cpus > 4) cpus = 4;
  if (gThread->multiThread(gain_rows, cpus, &j) != kOfxStatOK) return kOfxStatFailed;

  /* 目印: 正準座標（左下原点）の marker の位置に 1 画素。描画スケールを掛けて画素にする */
  {
    int px = (int)(mx * scale[0]), py = (int)(my * scale[1]);
    if (px >= 0 && px < j.w && py >= 0 && py < j.h) {
      float *d = (float *)((char *)j.dst + (long)py * j.dst_row) + px * 4;
      d[0] = 1.0f;
      d[1] = 0.0f;
      d[2] = 1.0f;
      d[3] = 1.0f;
    }
  }
  /* 文字列パラメータが "msg" ならメッセージを出す */
  if (note && strcmp(note, "msg") == 0) gMsg->message(fx, kOfxMessageMessage, "id", "gain=%.2f note=%s", j.gain, note);

  gFx->clipReleaseImage(src_img);
  gFx->clipReleaseImage(dst_img);
  return kOfxStatOK;
}

/* ---------- 1: echo ---------- */

static OfxStatus echo_describe_in_context(OfxImageEffectHandle fx) {
  OfxParamSetHandle ps;
  OfxPropertySetHandle p;
  define_clips(fx, 1);
  gFx->getParamSet(fx, &ps);
  gParam->paramDefine(ps, kOfxParamTypeDouble, "level", &p);
  gProp->propSetDouble(p, kOfxParamPropDefault, 0, 1.0);
  return kOfxStatOK;
}

/* 出力 = 現在の入力 × level(t) ＋ 1 フレーム前の入力 × level(t-1) */
static OfxStatus echo_render(OfxImageEffectHandle fx, OfxPropertySetHandle in) {
  OfxImageClipHandle src_clip, dst_clip;
  OfxPropertySetHandle cur_img, prev_img, dst_img;
  OfxParamSetHandle ps;
  OfxParamHandle h;
  OfxRectI b;
  OfxTime time;
  void *cp, *pp = NULL, *dp;
  int row, prow, drow, x, y, w, hgt;
  double level_now, level_prev;
  int has_prev;

  gProp->propGetDouble(in, kOfxPropTime, 0, &time);
  gFx->clipGetHandle(fx, kOfxImageEffectSimpleSourceClipName, &src_clip, NULL);
  gFx->clipGetHandle(fx, kOfxImageEffectOutputClipName, &dst_clip, NULL);
  if (image_of(src_clip, time, &cur_img, &cp, &b, &row) != kOfxStatOK) return kOfxStatFailed;
  has_prev = image_of(src_clip, time - 1.0, &prev_img, &pp, &b, &prow) == kOfxStatOK;
  if (image_of(dst_clip, time, &dst_img, &dp, &b, &drow) != kOfxStatOK) return kOfxStatFailed;
  gFx->getParamSet(fx, &ps);
  gParam->paramGetHandle(ps, "level", &h, NULL);
  gParam->paramGetValueAtTime(h, time, &level_now);
  gParam->paramGetValueAtTime(h, time - 1.0, &level_prev);
  w = b.x2 - b.x1;
  hgt = b.y2 - b.y1;
  for (y = 0; y < hgt; y++) {
    const float *c = (const float *)((const char *)cp + (long)y * row);
    const float *p = has_prev ? (const float *)((const char *)pp + (long)y * prow) : NULL;
    float *d = (float *)((char *)dp + (long)y * drow);
    for (x = 0; x < w * 4; x++) d[x] = (float)(c[x] * level_now + (p ? p[x] * level_prev : 0.0));
  }
  gFx->clipReleaseImage(cur_img);
  if (has_prev) gFx->clipReleaseImage(prev_img);
  gFx->clipReleaseImage(dst_img);
  return kOfxStatOK;
}

/* ---------- 2: bytes ---------- */

static OfxStatus bytes_render(OfxImageEffectHandle fx, OfxPropertySetHandle in) {
  OfxImageClipHandle src_clip, dst_clip;
  OfxPropertySetHandle src_img, dst_img;
  OfxRectI b;
  OfxTime time;
  void *sp, *dp;
  char *depth = NULL;
  int srow, drow, x, y, w, hgt;
  gProp->propGetDouble(in, kOfxPropTime, 0, &time);
  gFx->clipGetHandle(fx, kOfxImageEffectSimpleSourceClipName, &src_clip, NULL);
  gFx->clipGetHandle(fx, kOfxImageEffectOutputClipName, &dst_clip, NULL);
  if (image_of(src_clip, time, &src_img, &sp, &b, &srow) != kOfxStatOK) return kOfxStatFailed;
  if (image_of(dst_clip, time, &dst_img, &dp, &b, &drow) != kOfxStatOK) return kOfxStatFailed;
  gProp->propGetString(src_img, kOfxImageEffectPropPixelDepth, 0, &depth);
  if (!depth || strcmp(depth, kOfxBitDepthByte) != 0) return kOfxStatErrImageFormat;
  w = b.x2 - b.x1;
  hgt = b.y2 - b.y1;
  if (srow != w * 4) return kOfxStatErrImageFormat;
  for (y = 0; y < hgt; y++) {
    const unsigned char *s = (const unsigned char *)sp + (long)y * srow;
    unsigned char *d = (unsigned char *)dp + (long)y * drow;
    for (x = 0; x < w; x++) {
      /* 赤と青を入れ替える */
      d[x * 4] = s[x * 4 + 2];
      d[x * 4 + 1] = s[x * 4 + 1];
      d[x * 4 + 2] = s[x * 4];
      d[x * 4 + 3] = s[x * 4 + 3];
    }
  }
  gFx->clipReleaseImage(src_img);
  gFx->clipReleaseImage(dst_img);
  return kOfxStatOK;
}

/* ---------- 入口 ---------- */

static int gInstances[4];

static OfxStatus entry(int which, const char *action, const void *handle, OfxPropertySetHandle in, OfxPropertySetHandle out) {
  OfxImageEffectHandle fx = (OfxImageEffectHandle)handle;
  (void)out;
  if (strcmp(action, kOfxActionLoad) == 0) return fetch_suites();
  if (strcmp(action, kOfxActionUnload) == 0) return kOfxStatOK;
  if (strcmp(action, kOfxActionDescribe) == 0) {
    switch (which) {
    case 0: return describe_common(fx, "テスト: ゲイン", kOfxBitDepthFloat, kOfxImageEffectContextFilter);
    case 1: return describe_common(fx, "テスト: エコー", kOfxBitDepthFloat, kOfxImageEffectContextFilter);
    case 2: return describe_common(fx, "テスト: 8bit", kOfxBitDepthByte, kOfxImageEffectContextFilter);
    default: return describe_common(fx, "テスト: ジェネレーター", kOfxBitDepthFloat, kOfxImageEffectContextGenerator);
    }
  }
  if (strcmp(action, kOfxImageEffectActionDescribeInContext) == 0) {
    char *context = NULL;
    gProp->propGetString(in, kOfxImageEffectPropContext, 0, &context);
    if (!context || strcmp(context, kOfxImageEffectContextFilter) != 0) return kOfxStatErrUnsupported;
    switch (which) {
    case 0: return gain_describe_in_context(fx);
    case 1: return echo_describe_in_context(fx);
    default: define_clips(fx, 0); return kOfxStatOK;
    }
  }
  if (strcmp(action, kOfxActionCreateInstance) == 0) {
    OfxPropertySetHandle props;
    gFx->getPropertySet(fx, &props);
    gInstances[which]++;
    /* インスタンスデータを置いて、描画のときに読み戻せること */
    gProp->propSetPointer(props, kOfxPropInstanceData, 0, &gInstances[which]);
    return kOfxStatOK;
  }
  if (strcmp(action, kOfxActionDestroyInstance) == 0) return kOfxStatOK;
  if (strcmp(action, kOfxImageEffectActionRender) == 0) {
    OfxPropertySetHandle props;
    void *data = NULL;
    gFx->getPropertySet(fx, &props);
    gProp->propGetPointer(props, kOfxPropInstanceData, 0, &data);
    if (data != &gInstances[which]) return kOfxStatErrFatal;
    switch (which) {
    case 0: return gain_render(fx, in);
    case 1: return echo_render(fx, in);
    case 2: return bytes_render(fx, in);
    default: return kOfxStatFailed;
    }
  }
  return kOfxStatReplyDefault;
}

static OfxStatus entry0(const char *a, const void *h, OfxPropertySetHandle i, OfxPropertySetHandle o) { return entry(0, a, h, i, o); }
static OfxStatus entry1(const char *a, const void *h, OfxPropertySetHandle i, OfxPropertySetHandle o) { return entry(1, a, h, i, o); }
static OfxStatus entry2(const char *a, const void *h, OfxPropertySetHandle i, OfxPropertySetHandle o) { return entry(2, a, h, i, o); }
static OfxStatus entry3(const char *a, const void *h, OfxPropertySetHandle i, OfxPropertySetHandle o) { return entry(3, a, h, i, o); }

static void set_host(OfxHost *host) { gHost = host; }

static OfxPlugin gPlugins[4] = {
    {kOfxImageEffectPluginApi, 1, "org.znd.test.gain", 1, 2, set_host, entry0},
    {kOfxImageEffectPluginApi, 1, "org.znd.test.echo", 1, 0, set_host, entry1},
    {kOfxImageEffectPluginApi, 1, "org.znd.test.bytes", 1, 0, set_host, entry2},
    {kOfxImageEffectPluginApi, 1, "org.znd.test.gen", 1, 0, set_host, entry3},
};

OfxExport int OfxGetNumberOfPlugins(void) { return 4; }
OfxExport OfxPlugin *OfxGetPlugin(int nth) { return nth >= 0 && nth < 4 ? &gPlugins[nth] : 0; }
