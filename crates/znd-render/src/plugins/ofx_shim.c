/* OpenFX のパラメータ・メッセージ関数は可変長引数（C の ...）で値を受け渡す。
 * 可変長引数の関数は Rust（安定版）では定義できないので、ここで受けて固定引数の Rust 関数へ渡す。 */
#include <stdarg.h>
#include <stdio.h>

typedef int OfxStatus;

enum { K_DOUBLE = 0, K_INT = 1, K_RGBA = 2, K_RGB = 3, K_DOUBLE2D = 4, K_INT2D = 5, K_DOUBLE3D = 6, K_INT3D = 7, K_STRING = 8 };

extern int znd_ofx_param_kind(void *param);
extern OfxStatus znd_ofx_param_get(void *param, int has_time, double time, double *out4, const char **str);
extern OfxStatus znd_ofx_param_set(void *param, const double *in4, const char *str);
extern OfxStatus znd_ofx_message_text(void *handle, const char *type, const char *id, const char *text);

static int count_of(int kind) {
  switch (kind) {
  case K_RGBA: return 4;
  case K_RGB: case K_DOUBLE3D: case K_INT3D: return 3;
  case K_DOUBLE2D: case K_INT2D: return 2;
  default: return 1;
  }
}

static OfxStatus get_common(void *param, int has_time, double time, va_list ap) {
  int kind = znd_ofx_param_kind(param);
  double v[4] = {0, 0, 0, 0};
  const char *s = "";
  OfxStatus st;
  int i, n;
  if (kind < 0) return 9; /* kOfxStatErrBadHandle */
  st = znd_ofx_param_get(param, has_time, time, v, &s);
  if (st != 0) return st;
  n = count_of(kind);
  if (kind == K_STRING) {
    const char **out = va_arg(ap, const char **);
    if (out) *out = s;
    return 0;
  }
  for (i = 0; i < n; i++) {
    if (kind == K_INT || kind == K_INT2D || kind == K_INT3D) {
      int *out = va_arg(ap, int *);
      if (out) *out = (int)(v[i] < 0 ? v[i] - 0.5 : v[i] + 0.5);
    } else {
      double *out = va_arg(ap, double *);
      if (out) *out = v[i];
    }
  }
  return 0;
}

OfxStatus znd_ofx_paramGetValue(void *param, ...) {
  va_list ap;
  OfxStatus st;
  va_start(ap, param);
  st = get_common(param, 0, 0.0, ap);
  va_end(ap);
  return st;
}

OfxStatus znd_ofx_paramGetValueAtTime(void *param, double time, ...) {
  va_list ap;
  OfxStatus st;
  va_start(ap, time);
  st = get_common(param, 1, time, ap);
  va_end(ap);
  return st;
}

static OfxStatus set_common(void *param, va_list ap) {
  int kind = znd_ofx_param_kind(param);
  double v[4] = {0, 0, 0, 0};
  int i, n;
  if (kind < 0) return 9;
  if (kind == K_STRING) return znd_ofx_param_set(param, v, va_arg(ap, const char *));
  n = count_of(kind);
  for (i = 0; i < n; i++) {
    if (kind == K_INT || kind == K_INT2D || kind == K_INT3D)
      v[i] = (double)va_arg(ap, int);
    else
      v[i] = va_arg(ap, double);
  }
  return znd_ofx_param_set(param, v, 0);
}

OfxStatus znd_ofx_paramSetValue(void *param, ...) {
  va_list ap;
  OfxStatus st;
  va_start(ap, param);
  st = set_common(param, ap);
  va_end(ap);
  return st;
}

OfxStatus znd_ofx_paramSetValueAtTime(void *param, double time, ...) {
  va_list ap;
  OfxStatus st;
  (void)time;
  va_start(ap, time);
  st = set_common(param, ap);
  va_end(ap);
  return st;
}

extern OfxStatus znd_ofx_param_derive(void *param, double time, double *out4);
extern OfxStatus znd_ofx_param_integrate(void *param, double time1, double time2, double *out4);

/* 微分・積分は実数のパラメータだけ（ホストが数値的に求める） */
static OfxStatus put_doubles(void *param, const double *v, va_list ap) {
  int kind = znd_ofx_param_kind(param);
  int i, n = count_of(kind);
  if (kind != K_DOUBLE && kind != K_RGBA && kind != K_RGB && kind != K_DOUBLE2D && kind != K_DOUBLE3D) return 5; /* kOfxStatErrUnsupported */
  for (i = 0; i < n; i++) {
    double *out = va_arg(ap, double *);
    if (out) *out = v[i];
  }
  return 0;
}

OfxStatus znd_ofx_paramGetDerivative(void *param, double time, ...) {
  double v[4] = {0, 0, 0, 0};
  va_list ap;
  OfxStatus st = znd_ofx_param_derive(param, time, v);
  if (st != 0) return st;
  va_start(ap, time);
  st = put_doubles(param, v, ap);
  va_end(ap);
  return st;
}

OfxStatus znd_ofx_paramGetIntegral(void *param, double time1, double time2, ...) {
  double v[4] = {0, 0, 0, 0};
  va_list ap;
  OfxStatus st = znd_ofx_param_integrate(param, time1, time2, v);
  if (st != 0) return st;
  va_start(ap, time2);
  st = put_doubles(param, v, ap);
  va_end(ap);
  return st;
}

OfxStatus znd_ofx_message(void *handle, const char *type, const char *id, const char *format, ...) {
  char text[1024];
  va_list ap;
  text[0] = 0;
  if (format) {
    va_start(ap, format);
    vsnprintf(text, sizeof text, format, ap);
    va_end(ap);
  }
  return znd_ofx_message_text(handle, type, id, text);
}
