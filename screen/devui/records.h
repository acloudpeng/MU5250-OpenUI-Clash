/* Proxy and Clash records from the agent, and text the panel can display.
 * Groups and nodes come with names but are only ever sent back as indices plus the
 * list generation, so no name has to be parsed by the agent.
 * Copyright (c) 2026 ZweiChen. GPL-3.0-or-later. */
#include <limits.h>
enum { BK_OFF, BK_WG, BK_CLASH };
/* A diagnosis figure the agent did not report. */
#define NO_FIGURE LLONG_MIN
struct backend_info {
  int has, active, conflict;
  unsigned long long rev[2]; /* WireGuard, Clash */
  int ready[2], error[2];
  char reason[2][12];
};
struct clash_info {
  int has, enabled, running, connected, installed;
  unsigned long long rev, uptime, memory_kib, active;
  char mode[8], version[33], error[97];
};
struct sub_info {
  unsigned long long id, updated, up, down, total, expire;
  int active, downloaded, proxies, failed;
  char name[193];
};
struct core_info {
  int has, available, newer;
  unsigned long long bytes, total, free;
  char version[33], source[13], previous[33], latest[33], published[11];
  char kind[13], state[13], step[17], error[97];
};
struct check_info { char id[13], status[9]; long long v[4]; };
struct diag_info {
  int has, running, n;
  unsigned long long finished;
  char verdict[17];
  struct check_info checks[8];
};
struct group_row { int index, selectable, members, now; char type[16], name[193], now_name[193]; };
/* state: -1 no answer, 0 loading, 1 ready, 2 Clash not running. */
struct groups_view { int state, total, offset, n; unsigned long long gen; struct group_row rows[4]; };
struct node_row { int index; long long delay; char type[16], name[193]; };
struct nodes_view {
  int ok, group, total, offset, n, selectable, now;
  unsigned long long gen;
  char type[16], name[193];
  struct node_row rows[4];
};
/* Unifont covers the BMP only. Flags (regional-indicator pairs) become their ASCII
 * letters, e.g. U+1F1ED U+1F1F0 -> "HK"; other characters outside the BMP, variation
 * selectors, joiners and broken UTF-8 are dropped, then spaces are tidied. In place. */
static void display_text(char *s) {
  const unsigned char *in = (const unsigned char *)s;
  char *out = s;
  while (*in) {
    unsigned cp;
    int len = in[0] < 0x80 ? 1 : (in[0] & 0xe0) == 0xc0 ? 2 : (in[0] & 0xf0) == 0xe0 ? 3 : (in[0] & 0xf8) == 0xf0 ? 4 : 0;
    if (!len) { in++; continue; }
    cp = len == 1 ? in[0] : in[0] & (0x7f >> len);
    int i = 1;
    for (; i < len && (in[i] & 0xc0) == 0x80; i++) cp = cp << 6 | (in[i] & 0x3f);
    if (i < len) { in++; continue; }
    if (cp >= 0x1f1e6 && cp <= 0x1f1ff) *out++ = (char)('A' + (cp - 0x1f1e6));
    else if (!(cp > 0xffff || (cp >= 0xfe00 && cp <= 0xfe0f) || cp == 0x200d || cp == 0x20e3)) {
      memmove(out, in, (size_t)len);
      out += len;
    }
    in += len;
  }
  *out = 0;
  char *r = s, *w = s;
  while (*r == ' ') r++;
  for (; *r; r++) if (*r != ' ' || w[-1] != ' ') *w++ = *r;
  while (w > s && w[-1] == ' ') w--;
  *w = 0;
}
/* Splits a record on tabs in place; the last field keeps any further tabs. */
static int split_tabs(char *line, char **f, int max) {
  int n = 0;
  while (n < max) {
    f[n++] = line;
    if (!(line = strchr(line, '\t'))) break;
    *line++ = 0;
  }
  return n;
}
/* The agent sends "-" for an empty field. Text longer than the buffer (the agent sizes
 * its fields to fit) ends before the first character that does not fit whole. */
static void copy_field(char *dst, size_t cap, const char *src) {
  if (!strcmp(src, "-")) src = "";
  size_t n = strlen(src);
  if (n >= cap)
    for (n = cap - 1; n > 0 && ((unsigned char)src[n] & 0xc0) == 0x80; n--) {}
  memcpy(dst, src, n);
  dst[n] = 0;
}
static void copy_name(char *dst, size_t cap, const char *src) { copy_field(dst, cap, src); display_text(dst); }
static unsigned long long u64_field(const char *s) { return strtoull(s, NULL, 10); }
static long long figure_field(const char *s) { return strcmp(s, "-") ? strtoll(s, NULL, 10) : NO_FIGURE; }
#define COPY(dst, src) copy_field(dst, sizeof(dst), src)
/* B active conflict, then per backend: revision ready reason error. */
static void parse_backend(char **f, int n, struct backend_info *b) {
  if (n < 11) return;
  b->active = !strcmp(f[1], "wireguard") ? BK_WG : !strcmp(f[1], "clash") ? BK_CLASH : BK_OFF;
  b->conflict = atoi(f[2]) != 0;
  for (int i = 0; i < 2; i++) {
    b->rev[i] = u64_field(f[3 + 4 * i]);
    b->ready[i] = atoi(f[4 + 4 * i]) != 0;
    COPY(b->reason[i], f[5 + 4 * i]);
    b->error[i] = atoi(f[6 + 4 * i]) != 0;
  }
  b->has = 1;
}
/* K revision enabled running connected mode uptime memory active installed version error */
static void parse_clash(char **f, int n, struct clash_info *c) {
  if (n < 12) return;
  c->rev = u64_field(f[1]);
  c->enabled = atoi(f[2]) != 0;
  c->running = atoi(f[3]) != 0;
  c->connected = atoi(f[4]) != 0;
  COPY(c->mode, f[5]);
  c->uptime = u64_field(f[6]);
  c->memory_kib = u64_field(f[7]);
  c->active = u64_field(f[8]);
  c->installed = atoi(f[9]) != 0;
  COPY(c->version, f[10]);
  COPY(c->error, f[11]);
  c->has = 1;
}
/* U id active downloaded updated upload download total expire proxies failed name */
static int parse_sub(char **f, int n, struct sub_info *s) {
  if (n < 12) return 0;
  s->id = u64_field(f[1]);
  if (!s->id) return 0;
  s->active = atoi(f[2]) != 0;
  s->downloaded = atoi(f[3]) != 0;
  s->updated = u64_field(f[4]);
  s->up = u64_field(f[5]);
  s->down = u64_field(f[6]);
  s->total = u64_field(f[7]);
  s->expire = u64_field(f[8]);
  s->proxies = atoi(f[9]);
  s->failed = atoi(f[10]) != 0;
  copy_name(s->name, sizeof(s->name), f[11]);
  return 1;
}
/* M available version source previous latest published newer kind state step bytes
 * total free error; "M 0" from an agent without core updates. */
static void parse_core(char **f, int n, struct core_info *c) {
  if (n < 2) return;
  c->has = 1;
  c->available = atoi(f[1]) != 0 && n >= 15;
  if (!c->available) return;
  COPY(c->version, f[2]);
  COPY(c->source, f[3]);
  COPY(c->previous, f[4]);
  COPY(c->latest, f[5]);
  COPY(c->published, f[6]);
  c->newer = atoi(f[7]) != 0;
  COPY(c->kind, f[8]);
  COPY(c->state, f[9]);
  COPY(c->step, f[10]);
  c->bytes = u64_field(f[11]);
  c->total = u64_field(f[12]);
  c->free = u64_field(f[13]);
  COPY(c->error, f[14]);
}
/* X running finished verdict, then x id status figure*4 */
static void parse_diag(char **f, int n, struct diag_info *d) {
  if (n < 4) return;
  d->has = 1;
  d->running = atoi(f[1]) != 0;
  d->finished = u64_field(f[2]);
  COPY(d->verdict, f[3]);
}
static void parse_check(char **f, int n, struct diag_info *d) {
  if (n < 7 || d->n >= 8) return;
  struct check_info *c = &d->checks[d->n++];
  COPY(c->id, f[1]);
  COPY(c->status, f[2]);
  for (int i = 0; i < 4; i++) c->v[i] = figure_field(f[3 + i]);
}
/* Reply to "groups <offset>": G state generation total offset, g rows. */
static int parse_groups(char *buf, struct groups_view *out) {
  struct groups_view v = {.state = -1};
  char *save, *line = strtok_r(buf, "\n", &save);
  for (; line; line = strtok_r(NULL, "\n", &save)) {
    char *f[8];
    int n = split_tabs(line, f, 8);
    if (!strcmp(f[0], "G") && n >= 5) {
      v.state = atoi(f[1]);
      v.gen = u64_field(f[2]);
      v.total = atoi(f[3]);
      v.offset = atoi(f[4]);
    } else if (!strcmp(f[0], "g") && n >= 8 && v.n < 4) {
      struct group_row *r = &v.rows[v.n++];
      r->index = atoi(f[1]);
      r->selectable = atoi(f[2]) != 0;
      r->members = atoi(f[3]);
      r->now = atoi(f[4]);
      COPY(r->type, f[5]);
      copy_name(r->name, sizeof(r->name), f[6]);
      copy_name(r->now_name, sizeof(r->now_name), f[7]);
    }
  }
  if (v.state < 0) return 0;
  *out = v;
  return 1;
}
/* Reply to "nodes <gen> <group> <offset>": N gen group total offset selectable now type
 * name, n index delay type name (delay -1 untested, 0 failed). */
static int parse_nodes(char *buf, struct nodes_view *out) {
  struct nodes_view v = {0};
  char *save, *line = strtok_r(buf, "\n", &save);
  for (; line; line = strtok_r(NULL, "\n", &save)) {
    char *f[9];
    int n = split_tabs(line, f, 9);
    if (!strcmp(f[0], "N") && n >= 9) {
      v.ok = 1;
      v.gen = u64_field(f[1]);
      v.group = atoi(f[2]);
      v.total = atoi(f[3]);
      v.offset = atoi(f[4]);
      v.selectable = atoi(f[5]) != 0;
      v.now = atoi(f[6]);
      COPY(v.type, f[7]);
      copy_name(v.name, sizeof(v.name), f[8]);
    } else if (!strcmp(f[0], "n") && n >= 5 && v.n < 4) {
      struct node_row *r = &v.rows[v.n++];
      r->index = atoi(f[1]);
      r->delay = strtoll(f[2], NULL, 10);
      COPY(r->type, f[3]);
      copy_name(r->name, sizeof(r->name), f[4]);
    }
  }
  if (!v.ok) return 0;
  *out = v;
  return 1;
}
