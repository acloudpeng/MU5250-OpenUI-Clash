/* SPDX-License-Identifier: GPL-3.0-or-later
 * WireGuard touch UI; native display and root IPC only. See LICENSE and NOTICE.
 */
#define _GNU_SOURCE
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdarg.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#ifndef SCREEN_PREVIEW
#include "qpic.h"
#include <dirent.h>
#include <sys/file.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <sys/wait.h>
#else
#define W 320
#define H 480
struct drm_buf {
  uint16_t *map;
  uint32_t pitch;
};
static void fill(struct drm_buf *b, uint16_t c) {
  for (int i = 0; i < W * H; i++)
    b->map[i] = c;
}
static void fill_rect(struct drm_buf *b, int x, int y, int x1, int y1,
                      uint16_t c) {
  for (int j = y; j < y1; j++)
    for (int i = x; i < x1; i++)
      if (i >= 0 && i < W && j >= 0 && j < H)
        b->map[j * W + i] = c;
}
static void draw_pixel(struct drm_buf *b, int x, int y, uint16_t c) {
  if (x >= 0 && x < W && y >= 0 && y < H)
    b->map[y * W + x] = c;
}
#endif
#include "power.h"
#include "devui/html.h"
#include "devui_assets.h"
#include "devui/records.h"
#define MAX_ROWS 128
#define BG 0x0863
#define CARD 0x1926
#define FG 0xef9e
#define MUTED 0x9d55
#define GREEN 0x46f3
#define BLUE 0x253b
#define RED 0xfa8a
#define RUNDIR "/tmp/openui-screen"
#define BL "/sys/class/leds/led:lcd/brightness"
struct row {
  char mac[18], ip[48], name[193];
  int selected, online;
};
struct profile { unsigned long long id; int active; char name[193]; };
struct snapshot {
  unsigned long long rev, handshake, rx, tx, stamp, job, done, fresh, failed_job;
  int has, enabled, connected, error, all, busy, result, n, has_fresh;
  int np, battery, temperature;
  unsigned long long settings_rev;
  int idle_seconds, brightness, home_page, menu_mask, zh, idle_sleep;
  unsigned long long uptime;
  double memory, cpu;
  struct profile profiles[5];
  /* Agents with proxy support: backend switch, Clash, subscriptions, core, diagnosis. */
  struct backend_info backend;
  struct clash_info clash;
  int nsubs;
  struct sub_info subs[5];
  struct core_info core;
  struct diag_info diag;
  struct esim_info esim;
  struct uplink_info uplink;
  struct toolbox_info toolbox;
  char failure[97]; /* the agent's message for failed_job */
  struct row rows[MAX_ROWS];
};
static struct snapshot data, draft;
static int original_selection[MAX_ROWS];
#ifndef SCREEN_PREVIEW
static int screen_awake = 1;
#endif
/* Menu features first (their numbers are the settings record's page bits), then pages
 * reached from them. */
enum { WG, DEVICES, MENU, PROFILES, OVERVIEW, PROXY, CLASH, ESIM, UPLINK, WIFI_PAGE, SIGNAL_PAGE, USB_PAGE, POWER_PAGE, BOOSTER_PAGE, NODES, GROUP, SUBS, CORE, DIAG };
#define LEGACY_PAGES 27  /* features of a C record: WireGuard, devices, profiles, overview */
#define MENU_PAGES 123   /* features of a V record: the above plus proxy and Clash */
#define ESIM_MENU_PAGES 251 /* features of a W record: the above plus eSIM */
#define UPLINK_MENU_PAGES 507 /* features of a Y record: the above plus the Wi-Fi uplink */
#define TOOLBOX_MENU_PAGES 16379 /* features of a Z record: the above plus Pocket OS toolbox pages */
static int page = MENU, offset, dialog, dirty, quitting, need_draw = 1,
                                                  return_after_save;
static unsigned long long settings_revision;
/* Until the first record: overview, proxy, WireGuard and Clash. */
static int settings_idle = 120, settings_brightness = 128, settings_home = MENU, settings_menu = 113;
/* Screen language follows the web panel (C record); Chinese until the first record. */
static int settings_zh = 1;
/* Idle action from the W record: 1 turns the screen off and stays in DevUI, 0 returns to
 * the stock UI (also with older agents, whose W records end at the language). */
static int settings_sleep = 0;
#define L(en, zh) (settings_zh ? (zh) : (en))
static int visible_page(int p) { return p == MENU || (p <= BOOSTER_PAGE && (settings_menu & (1 << p))); }
/* Saved profiles and device routing belong to WireGuard; node, subscription and core
 * pages to Clash; diagnostics to Clash and Proxy; the Wi-Fi uplink also to Overview,
 * which shows it when the agent reports one. */
static int reachable(int p) {
  switch (p) {
  case PROFILES: case DEVICES: return visible_page(p) || visible_page(WG);
  case NODES: case GROUP: case SUBS: case CORE: return visible_page(CLASH);
  case DIAG: return visible_page(CLASH) || visible_page(PROXY);
  case UPLINK: return visible_page(UPLINK) || (visible_page(OVERVIEW) && data.uplink.has);
  default: return visible_page(p);
  }
}
static int update_settings(const struct snapshot *s) {
  if (!s->settings_rev || s->settings_rev == settings_revision) return 0;
  int result = settings_revision ? 1 : 2;
  settings_revision = s->settings_rev;
  settings_idle = s->idle_seconds; settings_brightness = s->brightness;
  settings_home = s->home_page; settings_menu = s->menu_mask; settings_zh = s->zh;
  settings_sleep = s->idle_sleep;
  return result;
}
static unsigned long long pending, pending_revision, chosen_profile, chosen_revision;
static char chosen_name[193];
static char notice[128], failure_text[97];
/* What the pending job does, for its labels and its completion. */
enum { JOB_WG = 1, JOB_BACKEND, JOB_MODE, JOB_SUB, JOB_REFRESH, JOB_SELECT, JOB_DELAY, JOB_CORE, JOB_DIAG, JOB_ESIM, JOB_UPLINK, JOB_TOOL };
static int pending_kind;
/* When the panel stops waiting for the pending job's outcome. */
static long long pending_deadline;
static long long monotonic_ms(void) {
  struct timespec t;
  clock_gettime(CLOCK_MONOTONIC, &t);
  return (long long)t.tv_sec * 1000 + t.tv_nsec / 1000000;
}
/* How long a job's outcome may take: a switch can wait minutes for a backend's lock and
 * start, a download or core restart too; a node choice, delay test (20 s at most) or
 * the start of a diagnosis answers in seconds. Past it the job is taken as lost. */
static long long job_wait_ms(int kind) {
  /* An eSIM action only starts the agent's own job, which its Q record then follows. */
  return kind == JOB_SELECT || kind == JOB_DELAY || kind == JOB_DIAG || kind == JOB_ESIM || kind == JOB_UPLINK || kind == JOB_TOOL ? 60000 : 180000;
}
static void amount(char *out, size_t n, unsigned long long v) {
  if (v >= 1073741824)
    snprintf(out, n, "%.1f GB", (double)v / 1073741824);
  else if (v >= 1048576)
    snprintf(out, n, "%.1f MB", (double)v / 1048576);
  else
    snprintf(out, n, "%.1f KB", (double)v / 1024);
}
static int stale(void) {
  return !data.stamp || (unsigned long long)time(NULL) > data.stamp + 10;
}
static int blocked(void) { return pending || data.busy || stale(); }
/* A change to WireGuard (clash 0) or to Clash (1) is in progress: a backend switch is
 * both, the WireGuard verbs only WireGuard, mode and subscription jobs only Clash; delay
 * tests, node choices, core jobs and diagnoses neither. A busy agent counts for both. */
static int applying(int clash) {
  if (!pending) return data.busy;
  if (pending_kind == JOB_BACKEND) return 1;
  return clash ? pending_kind >= JOB_MODE && pending_kind <= JOB_REFRESH : pending_kind == JOB_WG;
}
/* Proxy groups and the open group's nodes, fetched while their pages are shown. */
static struct groups_view groups;
static struct nodes_view nodes;
static int group_offset, node_offset, open_group, diag_offset, diag_from = MENU, esim_offset, uplink_from = MENU;
static unsigned long long open_gen;
static long long page_fetch_at;
/* The confirmation dialog (5) and the subscription dialog (6). */
enum { CF_BACKEND = 1, CF_UPDATE, CF_ROLLBACK, CF_GEO, CF_ESIM, CF_REBOOT, CF_UPLINK };
static char confirm_req[96];
static int confirm_what, confirm_target, confirm_job;
static unsigned long long confirm_rev, chosen_sub, chosen_sub_rev;
#include "devui/render.h"
// The LCD and raw touch axes are mounted upside down relative to the case.
// Keep layout coordinates upright; transform only at the hardware boundary.
static void rotate_point(int *x, int *y) {
  *x = W - 1 - *x;
  *y = H - 1 - *y;
}
static void rotate_frame(struct drm_buf *b) {
  for (int y = 0; y < H / 2; y++) {
    uint16_t *top = (uint16_t *)((uint8_t *)b->map + y * b->pitch);
    uint16_t *bottom = (uint16_t *)((uint8_t *)b->map + (H - 1 - y) * b->pitch);
    for (int x = 0; x < W; x++) {
      uint16_t value = top[x];
      top[x] = bottom[W - 1 - x];
      bottom[W - 1 - x] = value;
    }
  }
}
static int parse_snapshot(char *buf, struct snapshot *out) {
  struct snapshot s = {.battery = -1, .temperature = -999, .memory = -1};
  char *save, *line = strtok_r(buf, "\n", &save);
  int valid = 0, v_ok = 0, v_idle = 0, v_level = 0, v_home = 0, v_mask = 0;
  int w_ok = 0, w_idle = 0, w_level = 0, w_home = 0, w_mask = 0, w_sleep = 0;
  int y_ok = 0, y_idle = 0, y_level = 0, y_home = 0, y_mask = 0, y_sleep = 0;
  int z_ok = 0, z_idle = 0, z_level = 0, z_home = 0, z_mask = 0, z_sleep = 0;
  unsigned long long v_rev = 0, w_rev = 0, y_rev = 0, z_rev = 0;
  char v_lang[4] = "zh", w_lang[4] = "zh", y_lang[4] = "zh", z_lang[4] = "zh";
  while (line) {
    if (line[0] == 'S' &&
        sscanf(line, "S %llu %d %d %d %d %llu %llu %llu %d %llu", &s.rev,
               &s.has, &s.enabled, &s.connected, &s.error, &s.handshake, &s.rx,
               &s.tx, &s.all, &s.stamp) == 10)
      valid = 1;
    else if (line[0] == 'C') {
      unsigned long long rev; int idle, level, home, mask, fields; char lang[4] = "zh";
      /* The language field is optional: an older agent sends five. */
      if ((fields = sscanf(line, "C %llu %d %d %d %d %3s", &rev, &idle, &level, &home, &mask, lang)) >= 5 &&
          (fields == 5 || !strcmp(lang, "zh") || !strcmp(lang, "en")) &&
          idle >= 30 && idle <= 600 && level >= 20 && level <= 255 &&
          home >= 0 && home <= 4 && mask > 0 && !(mask & ~LEGACY_PAGES) &&
          (home == MENU || (mask & (1 << home)))) {
        s.settings_rev = rev; s.idle_seconds = idle; s.brightness = level;
        s.home_page = home; s.menu_mask = mask; s.zh = strcmp(lang, "en") != 0;
      }
    }
    /* V: the same settings with Proxy and Clash; preferred over C when valid. */
    else if (line[0] == 'V') {
      char lang[4] = "";
      if (sscanf(line, "V %llu %d %d %d %d %3s", &v_rev, &v_idle, &v_level, &v_home, &v_mask, lang) == 6 &&
          (!strcmp(lang, "zh") || !strcmp(lang, "en")) &&
          v_idle >= 30 && v_idle <= 600 && v_level >= 20 && v_level <= 255 &&
          v_home >= 0 && v_home <= CLASH && v_mask > 0 && !(v_mask & ~MENU_PAGES) &&
          (v_home == MENU || (v_mask & (1 << v_home)))) {
        v_ok = 1;
        snprintf(v_lang, sizeof(v_lang), "%s", lang);
      }
    }
    /* W: the same with eSIM and, from newer agents, the idle action; preferred over V. */
    else if (line[0] == 'W') {
      char lang[4] = "";
      int fields = sscanf(line, "W %llu %d %d %d %d %3s %d", &w_rev, &w_idle, &w_level, &w_home, &w_mask, lang, &w_sleep);
      if (fields == 6) w_sleep = 0;
      if ((fields == 6 || (fields == 7 && (w_sleep == 0 || w_sleep == 1))) &&
          (!strcmp(lang, "zh") || !strcmp(lang, "en")) &&
          w_idle >= 30 && w_idle <= 600 && w_level >= 20 && w_level <= 255 &&
          w_home >= 0 && w_home <= ESIM && w_mask > 0 && !(w_mask & ~ESIM_MENU_PAGES) &&
          (w_home == MENU || (w_mask & (1 << w_home)))) {
        w_ok = 1;
        snprintf(w_lang, sizeof(w_lang), "%s", lang);
      }
    }
    /* Y: the same with the Wi-Fi uplink; preferred over W. */
    else if (line[0] == 'Y') {
      char lang[4] = "";
      if (sscanf(line, "Y %llu %d %d %d %d %3s %d", &y_rev, &y_idle, &y_level, &y_home, &y_mask, lang, &y_sleep) == 7 &&
          (y_sleep == 0 || y_sleep == 1) && (!strcmp(lang, "zh") || !strcmp(lang, "en")) &&
          y_idle >= 30 && y_idle <= 600 && y_level >= 20 && y_level <= 255 &&
          y_home >= 0 && y_home <= UPLINK && y_mask > 0 && !(y_mask & ~UPLINK_MENU_PAGES) &&
          (y_home == MENU || (y_mask & (1 << y_home)))) {
        y_ok = 1;
        snprintf(y_lang, sizeof(y_lang), "%s", lang);
      }
    }
    /* Z: the same with Pocket OS toolbox pages; preferred over Y. */
    else if (line[0] == 'Z') {
      char lang[4] = "";
      if (sscanf(line, "Z %llu %d %d %d %d %3s %d", &z_rev, &z_idle, &z_level, &z_home, &z_mask, lang, &z_sleep) == 7 &&
          (z_sleep == 0 || z_sleep == 1) && (!strcmp(lang, "zh") || !strcmp(lang, "en")) &&
          z_idle >= 30 && z_idle <= 600 && z_level >= 20 && z_level <= 255 &&
          z_home >= 0 && z_home <= BOOSTER_PAGE && z_mask > 0 && !(z_mask & ~TOOLBOX_MENU_PAGES) &&
          (z_home == MENU || (z_mask & (1 << z_home)))) {
        z_ok = 1;
        snprintf(z_lang, sizeof(z_lang), "%s", lang);
      }
    }
    else if (line[0] == 'J')
      /* fresh, the newest job the records already show, is absent from older agents. */
      s.has_fresh = sscanf(line, "J %d %llu %llu %d %llu", &s.busy, &s.job, &s.done, &s.result, &s.fresh) == 5;
    else if (line[0] && strchr("BKUMXxEQqRH", line[0]) && line[1] == '\t') {
      char *f[36];
      int n = split_tabs(line, f, 36);
      switch (f[0][0]) {
      case 'B': parse_backend(f, n, &s.backend); break;
      case 'K': parse_clash(f, n, &s.clash); break;
      case 'U': if (s.nsubs < 5 && parse_sub(f, n, &s.subs[s.nsubs])) s.nsubs++; break;
      case 'M': parse_core(f, n, &s.core); break;
      case 'X': parse_diag(f, n, &s.diag); break;
      case 'x': parse_check(f, n, &s.diag); break;
      case 'E': if (n >= 3) { s.failed_job = u64_field(f[1]); COPY(s.failure, f[2]); } break;
      case 'Q': parse_esim(f, n, &s.esim); break;
      case 'q': parse_esim_row(f, n, &s.esim); break;
      case 'R': parse_uplink(f, n, &s.uplink); break;
      case 'H': parse_toolbox(f, n, &s.toolbox); break;
      }
    }
    else if (line[0] == 'O')
      sscanf(line, "O %d %d %llu %lf %lf", &s.battery, &s.temperature, &s.uptime, &s.memory, &s.cpu);
    else if (line[0] == 'P' && s.np < 5) {
      struct profile *p = &s.profiles[s.np];
      if (sscanf(line, "P\t%llu\t%d\t%192[^\n]", &p->id, &p->active, p->name) == 3 && p->id)
        s.np++;
    }
    else if (line[0] == 'D'  && s.n < MAX_ROWS) {
      struct row *r = &s.rows[s.n];
      char *sv, *p = strtok_r(line, "\t", &sv);
      int i = 0;
      while ((p = strtok_r(NULL, "\t", &sv))) {
        switch (i++) {
        case 0:
          snprintf(r->mac, sizeof(r->mac), "%s", p);
          break;
        case 1:
          r->selected = atoi(p) != 0;
          break;
        case 2:
          r->online = atoi(p) != 0;
          break;
        case 3:
          snprintf(r->ip, sizeof(r->ip), "%s", p);
          break;
        case 4:
          snprintf(r->name, sizeof(r->name), "%s", p);
          break;
        }
      }
      if (i == 5 && strlen(r->mac) == 17)
        s.n++;
    }
    line = strtok_r(NULL, "\n", &save);
  }
  if (z_ok) {
    s.settings_rev = z_rev; s.idle_seconds = z_idle; s.brightness = z_level; s.idle_sleep = z_sleep;
    s.home_page = z_home; s.menu_mask = z_mask; s.zh = strcmp(z_lang, "en") != 0;
  } else if (y_ok) {
    s.settings_rev = y_rev; s.idle_seconds = y_idle; s.brightness = y_level; s.idle_sleep = y_sleep;
    s.home_page = y_home; s.menu_mask = y_mask; s.zh = strcmp(y_lang, "en") != 0;
  } else if (w_ok) {
    s.settings_rev = w_rev; s.idle_seconds = w_idle; s.brightness = w_level; s.idle_sleep = w_sleep;
    s.home_page = w_home; s.menu_mask = w_mask; s.zh = strcmp(w_lang, "en") != 0;
  } else if (v_ok) {
    s.settings_rev = v_rev; s.idle_seconds = v_idle; s.brightness = v_level;
    s.home_page = v_home; s.menu_mask = v_mask; s.zh = strcmp(v_lang, "en") != 0;
  }
  if (valid || s.settings_rev)
    *out = s;
  return valid || s.settings_rev;
}
#ifndef SCREEN_PREVIEW
static int exchange(const char *request, char *reply, size_t cap) {
  int fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
  if (fd < 0)
    return -1;
  struct timeval t = {.tv_sec = 0, .tv_usec = 300000};
  setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &t, sizeof(t));
  setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &t, sizeof(t));
  struct sockaddr_un a = {.sun_family = AF_UNIX};
  strcpy(a.sun_path, RUNDIR "/control.sock");
  int ok = -1;
  if (connect(fd, (void *)&a, sizeof(a)) == 0 &&
      send(fd, request, strlen(request), MSG_NOSIGNAL) ==
          (ssize_t)strlen(request)) {
    size_t n = 0;
    ssize_t k;
    while (n + 1 < cap && (k = read(fd, reply + n, cap - n - 1)) > 0)
      n += (size_t)k;
    reply[n] = 0;
    if (n > 0)
      ok = 0;
  }
  close(fd);
  return ok;
}
#else
/* Preview: the last request, all requests since the log was cleared, canned replies. */
static char test_request[4096], test_log[8192];
static const char *stub_groups, *stub_nodes, *stub_job = "OK 1\n";
static int exchange(const char *r, char *out, size_t cap) {
  snprintf(test_request, sizeof(test_request), "%s", r);
  size_t used = strlen(test_log);
  snprintf(test_log + used, sizeof(test_log) - used, "%s", r);
  const char *reply = !strncmp(r, "groups ", 7) ? stub_groups : !strncmp(r, "nodes ", 6) ? stub_nodes : stub_job;
  if (!reply) return -1;
  snprintf(out, cap, "%s", reply);
  return 0;
}
#endif
static void fetch_groups(void);
static int parent(int p);
/* Starts an agent job: one at a time, answered with its number. A stale node list is
 * reloaded; other refusals are explained in a dialog. */
static int send_job(const char *req, int kind, unsigned long long revision) {
  char reply[128] = "";
  if (blocked())
    return -1;
  pending_revision = revision;
  if (exchange(req, reply, sizeof(reply)) == 0 && sscanf(reply, "OK %llu", &pending) == 1 && pending) {
    pending_kind = kind;
    pending_deadline = monotonic_ms() + job_wait_ms(kind);
    dialog = 0;
    notice[0] = 0;
    need_draw = 1;
    return 0;
  }
  pending = 0;
  pending_kind = 0;
  failure_text[0] = 0;
  if (!strncmp(reply, "ERR Stale", 9) && kind == JOB_ESIM) {
    /* The card was read again meanwhile: the page shows the new list. */
    snprintf(notice, sizeof(notice), "%s", L("The profile list changed. Check it again.", "配置文件列表已变化，请重新确认。"));
    dialog = 0;
  } else if (!strncmp(reply, "ERR Stale", 9)) {
    snprintf(notice, sizeof(notice), "%s", L("The node list changed. Reloaded.", "节点列表已变化，已重新加载。"));
    dialog = 0;
    if (page == GROUP) page = NODES;
    fetch_groups();
  } else {
    snprintf(notice, sizeof(notice), "%s", !strncmp(reply, "ERR Busy", 8)
        ? L("Another action is still running.", "另一项操作仍在进行。")
        : L("Request failed. Try again.", "请求失败，请重试。"));
    dialog = 3;
  }
  need_draw = 1;
  return -1;
}
static void submit(int toggle, int value) {
  char req[4096];
  if (blocked())
    return;
  int kind = JOB_WG;
  if (toggle == 2)
    snprintf(req, sizeof(req), "profile %llu %llu\n", chosen_revision, chosen_profile);
  else if (toggle && data.backend.has) {
    /* The backend switch turns Clash off in the same transaction; toggle cannot. Off
     * names WireGuard, the backend this page shows as on. */
    snprintf(req, sizeof(req), "backend %llu %s\n", data.rev, value ? "wireguard" : "off wireguard");
    kind = JOB_BACKEND;
  } else if (toggle)
    snprintf(req, sizeof(req), "toggle %llu %d\n", data.rev, value);
  else {
    size_t n = (size_t)snprintf(req, sizeof(req), "devices %llu ", draft.rev);
    int count = 0;
    for (int i = 0; i < draft.n; i++) {
      // Send all draft entries on version conflict; agent rejects the stale
      // revision atomically.
      n += (size_t)snprintf(req + n, sizeof(req) - n, "%s%s=%d",
                            count++ ? "," : "", draft.rows[i].mac,
                            draft.rows[i].selected);
    }
    if (!count) {
      dirty = 0;
      page = parent(DEVICES);
      return;
    }
    snprintf(req + n, sizeof(req) - n, "\n");
  }
  send_job(req, kind, toggle == 2 ? chosen_revision : toggle ? data.rev : draft.rev);
}
#include "devui/actions.h"
static const char *job_failure(int kind, int result) {
  if (result == 1)
    return kind == JOB_CORE ? L("A core job or deployment is running.", "内核任务或部署正在进行。")
         : kind == JOB_ESIM ? L("An eSIM job is already running.", "已有 eSIM 任务在运行。")
                            : L("Settings changed. Refresh.", "设置已被更改，请刷新。");
  switch (kind) {
  case JOB_DELAY: return L("Delay test failed.", "测速失败。");
  case JOB_SELECT: return L("Could not select the node.", "无法选择该节点。");
  case JOB_REFRESH: return L("Subscription update failed.", "订阅更新失败。");
  case JOB_CORE: return L("The core request was refused.", "内核请求被拒绝。");
  case JOB_DIAG: return L("Diagnostics could not start.", "无法启动诊断。");
  case JOB_ESIM: return L("The eSIM request was refused.", "eSIM 请求被拒绝。");
  case JOB_UPLINK: return L("The Wi-Fi relay could not be switched.", "无法切换 Wi-Fi 中继。");
  default: return L("Apply failed. Check OpenUI.", "应用失败，请在 OpenUI 中检查。");
  }
}
/* A finished job ends the pending state once the records show its outcome. Agents
 * before the J record's fresh field only signal WireGuard jobs, by a higher revision. */
static void settle_jobs(void) {
  if (!pending)
    return;
  /* The agent numbers jobs from 1 after a restart: the pending job's outcome is lost. */
  if (data.job < pending) {
    pending = 0;
    pending_kind = 0;
    snprintf(notice, sizeof(notice), "%s", L("The agent restarted. Check the result.", "Agent 已重启，请检查结果。"));
    return;
  }
  int kind = pending_kind;
  int finished = data.done >= pending && (!data.has_fresh || data.fresh >= pending);
  if (finished && data.result) {
    snprintf(failure_text, sizeof(failure_text), "%s", data.failed_job == pending ? data.failure : "");
    snprintf(notice, sizeof(notice), "%s", job_failure(kind, data.result));
    pending = 0;
    pending_kind = 0;
    dialog = 3;
    return;
  }
  if (!finished || (!data.has_fresh && data.rev <= pending_revision)) {
    /* An outcome that never comes must not block the panel. */
    if (monotonic_ms() >= pending_deadline) {
      failure_text[0] = 0;
      snprintf(notice, sizeof(notice), "%s", L("No result from the agent in time. Check the result.", "Agent 未及时返回结果，请检查。"));
      pending = 0;
      pending_kind = 0;
      return_after_save = 0;
      dialog = 3;
    }
    return;
  }
  pending = 0;
  pending_kind = 0;
  notice[0] = 0;
  if (kind == JOB_WG) {
    dirty = 0;
    if (page == DEVICES) {
      draft = data;
      for (int i = 0; i < draft.n; i++)
        original_selection[i] = draft.rows[i].selected;
      if (return_after_save)
        page = parent(DEVICES);
    }
    return_after_save = 0;
  }
  if (kind == JOB_SELECT || kind == JOB_DELAY)
    refresh_page();
}
/* After each status: settings, the device draft, pages that are no longer shown, jobs. */
static void track_status(int changed) {
  if (changed == 2) {
    page = settings_home;
    if (page == DEVICES) enter_devices();
    else refresh_page();
  }
  if (page == DEVICES && !dirty && !pending && !dialog) {
    draft = data;
    for (int i = 0; i < draft.n; i++) original_selection[i] = draft.rows[i].selected;
    if (offset >= draft.n) offset = 0;
  }
  /* A shorter check list (a new run clears it) keeps diagnostics on a page it has. */
  int last_checks = data.diag.n > 0 ? (data.diag.n - 1) / 4 * 4 : 0;
  if (diag_offset > last_checks) diag_offset = last_checks;
  /* Likewise eSIM profiles, after one was deleted or downloaded on the webpage. */
  if (esim_offset >= data.esim.n) esim_offset = data.esim.n > 0 ? (data.esim.n - 1) / 4 * 4 : 0;
  if (!reachable(page) && !dirty && !pending && !dialog) page = MENU;
  settle_jobs();
  need_draw = 1;
}
#ifndef SCREEN_PREVIEW
static void putfile(const char *path, const char *s) {
  char tmp[256];
  snprintf(tmp, sizeof(tmp), "%s.new", path);
  int fd = open(tmp, O_CREAT | O_WRONLY | O_TRUNC | O_CLOEXEC, 0600);
  if (fd >= 0) {
    if (write(fd, s, strlen(s)) == (ssize_t)strlen(s)) {
      fsync(fd);
      rename(tmp, path);
    }
    close(fd);
  }
}
static int readint(const char *path) {
  FILE *f = fopen(path, "r");
  int v = -1;
  if (f) {
    if (fscanf(f, "%d", &v) != 1)
      v = -1;
    fclose(f);
  }
  return v;
}
static void brightness(int v) {
  int fd = open(BL, O_WRONLY | O_CLOEXEC);
  if (fd >= 0) {
    char b[16];
    int n = snprintf(b, sizeof(b), "%d", v);
    (void)!write(fd, b, (size_t)n);
    close(fd);
  }
}
/* Observe the dedicated key device without grabbing it: factory long-press
 * handling remains available. Never open the touchscreen as the power key. */
static int power_open(void) {
  for (int i = 0; i < 32; i++) {
    char path[64], name[128] = {0};
    snprintf(path, sizeof(path), "/dev/input/event%d", i);
    int fd = open(path, O_RDONLY | O_NONBLOCK | O_CLOEXEC);
    if (fd < 0) continue;
    unsigned char keys[(KEY_MAX + 8) / 8] = {0};
    if (ioctl(fd, EVIOCGNAME(sizeof(name) - 1), name) >= 0 &&
        !strcmp(name, "pmic_pwrkey") &&
        ioctl(fd, EVIOCGBIT(EV_KEY, sizeof(keys)), keys) >= 0 &&
        (keys[KEY_POWER / 8] & (1u << (KEY_POWER % 8)))) {
      return fd;
    }
    close(fd);
  }
  return -1;
}
static int power_check;
static int screen_toggle(int *on_level) {
  int current = readint(BL);
  if (screen_awake && current > 0) *on_level = current;
  int target = screen_awake ? 0 : *on_level;
  brightness(target);
  if (readint(BL) != target) {
    logline("power key: backlight write failed");
    return -1;
  }
  screen_awake = !screen_awake;
  need_draw = 1;
  logline("power key: display %s", screen_awake ? "awake" : "asleep");
  return 0;
}
static int processes(const char *name, int sig) {
  DIR *dir = opendir("/proc");
  if (!dir)
    return 0;
  struct dirent *e;
  int count = 0;
  while ((e = readdir(dir))) {
    int pid = atoi(e->d_name);
    if (pid <= 1)
      continue;
    char p[128], exe[256];
    snprintf(p, sizeof(p), "/proc/%d/exe", pid);
    ssize_t n = readlink(p, exe, sizeof(exe) - 1);
    if (n < 0)
      continue;
    exe[n] = 0;
    char *base = strrchr(exe, '/');
    if (base && !strcmp(base + 1, name)) {
      count++;
      if (sig)
        kill(pid, sig);
    }
  }
  closedir(dir);
  return count;
}
static int run(char *const args[], char *out, size_t cap) {
  int fds[2];
  if (pipe2(fds, O_CLOEXEC | O_NONBLOCK) < 0)
    return -1;
  pid_t pid = fork();
  if (pid < 0) {
    close(fds[0]);
    close(fds[1]);
    return -1;
  }
  if (!pid) {
    setpgid(0, 0);
    dup2(fds[1], 1);
    int n = open("/dev/null", O_RDWR);
    if (n >= 0) {
      dup2(n, 0);
      dup2(n, 2);
    }
    close(fds[0]);
    close(fds[1]);
    execvp(args[0], args);
    _exit(127);
  }
  close(fds[1]);
  setpgid(pid, pid);
  long long end = monotonic_ms() + 6000;
  int status = 0, done = 0;
  size_t used = 0;
  while (monotonic_ms() < end) {
    char buf[512];
    ssize_t n = read(fds[0], buf, sizeof(buf));
    if (n > 0 && out && used + 1 < cap) {
      size_t copy = (size_t)n < cap - used - 1 ? (size_t)n : cap - used - 1;
      memcpy(out + used, buf, copy);
      used += copy;
    }
    if (waitpid(pid, &status, WNOHANG) == pid) {
      done = 1;
      break;
    }
    usleep(20000);
  }
  if (!done) {
    kill(-pid, SIGKILL);
    waitpid(pid, &status, 0);
  }
  if (out && cap) {
    ssize_t n;
    while (used + 1 < cap && (n = read(fds[0], out + used, cap - used - 1)) > 0)
      used += (size_t)n;
  }
  if (out && cap)
    out[used] = 0;
  close(fds[0]);
  return done && WIFEXITED(status) ? WEXITSTATUS(status) : -1;
}
static int restore(void) {
  putfile(RUNDIR "/state", "restoring");
  int bl = readint(RUNDIR "/brightness"), touch = readint(RUNDIR "/touch"),
      ui = readint(RUNDIR "/ui");
  int ok = 1;
  if (ui == 1 && !processes("zte_topsw_devui", 0)) {
    char *args[] = {"/etc/init.d/zte_topsw_devui", "start", NULL};
    if (run(args, NULL, 0))
      ok = 0;
    for (int i = 0; i < 30 && !processes("zte_topsw_devui", 0); i++)
      usleep(100000);
    if (!processes("zte_topsw_devui", 0))
      ok = 0;
  }
  if (touch == 1 && !processes("mtdev2tuio", 0)) {
    pid_t p = fork();
    if (!p) {
      setsid();
      int n = open("/dev/null", O_RDWR);
      if (n >= 0) {
        dup2(n, 0);
        dup2(n, 1);
        dup2(n, 2);
      }
      execlp("mtdev2tuio", "mtdev2tuio", "/dev/input/event3",
             "osc.udp://127.0.0.1:3333/", (char *)NULL);
      _exit(127);
    }
    usleep(300000);
    if (!processes("mtdev2tuio", 0))
      ok = 0;
  }
  if (bl >= 0 && bl <= 255)
    brightness(bl);
  // Stock UI resumes its own sleep/brightness policy after ownership is
  // returned.
  char restored[16];
  snprintf(restored, sizeof(restored), "%d", readint(BL));
  putfile(RUNDIR "/brightness-restored", restored);
  putfile(RUNDIR "/state", ok ? "idle" : "error");
  return ok ? 0 : 1;
}
struct input_touch {
  int fd, slot, id[2], x, y, xok, yok;
  struct input_absinfo ax, ay;
  int down, sx, sy, moved, multi, dropped, suppress;
};
static int input_open(struct input_touch *t) {
  memset(t, 0, sizeof(*t));
  t->fd = -1;
  t->id[0] = t->id[1] = -1;
  for (int i = 0; i < 16; i++) {
    char p[64], name[128] = {0};
    snprintf(p, sizeof(p), "/dev/input/event%d", i);
    int fd = open(p, O_RDONLY | O_CLOEXEC | O_NONBLOCK);
    if (fd < 0)
      continue;
    if (ioctl(fd, EVIOCGNAME(sizeof(name) - 1), name) >= 0 &&
        strstr(name, "sitronix") &&
        ioctl(fd, EVIOCGABS(ABS_MT_POSITION_X), &t->ax) == 0 &&
        ioctl(fd, EVIOCGABS(ABS_MT_POSITION_Y), &t->ay) == 0) {
      t->fd = fd;
      break;
    }
    close(fd);
  }
  return t->fd >= 0 && t->ax.maximum > t->ax.minimum &&
                 t->ay.maximum > t->ay.minimum
             ? 0
             : -1;
}
static int axis(int v, struct input_absinfo *a, int size) {
  long n = (long)(v - a->minimum) * (size - 1) / (a->maximum - a->minimum);
  return n < 0 ? 0 : n >= size ? size - 1 : (int)n;
}
static int input_event(struct input_touch *t, struct input_event *e) {
  if (e->type == EV_SYN && e->code == SYN_DROPPED) {
    t->dropped = 1;
    t->down = 0;
    t->id[0] = t->id[1] = -1;
    return 0;
  }
  if (t->dropped) {
    if (e->type == EV_SYN && e->code == SYN_REPORT) {
      t->dropped = 0;
      t->xok = t->yok = 0;
    }
    return 0;
  }
  if (e->type == EV_ABS) {
    if (e->code == ABS_MT_SLOT)
      t->slot = e->value;
    else if (t->slot >= 0 && t->slot < 2) {
      if (e->code == ABS_MT_TRACKING_ID) {
        t->id[t->slot] = e->value;
        if (e->value >= 0 && t->slot == 0)
          t->xok = t->yok = 0;
      } else if (t->slot == 0 && e->code == ABS_MT_POSITION_X) {
        t->x = e->value;
        t->xok = 1;
      } else if (t->slot == 0 && e->code == ABS_MT_POSITION_Y) {
        t->y = e->value;
        t->yok = 1;
      }
    }
  }
  if (e->type == EV_KEY && e->code == BTN_TOUCH && e->value == 0)
    t->id[0] = t->id[1] = -1;
  if (e->type != EV_SYN || e->code != SYN_REPORT)
    return 0;
  int x = axis(t->x, &t->ax, W), y = axis(t->y, &t->ay, H);
  rotate_point(&x, &y);
  if (t->id[1] >= 0)
    t->multi = 1;
  if (t->id[0] >= 0 && t->xok && t->yok) {
    if (!t->down) {
      t->down = 1;
      t->sx = x;
      t->sy = y;
      t->moved = 0;
      t->multi = t->id[1] >= 0;
    }
    if (!screen_awake || t->suppress) t->multi = 1;
    if (abs(x - t->sx) > 12 || abs(y - t->sy) > 12)
      t->moved = 1;
  } else if (t->down && t->id[0] < 0) {
    if (screen_awake && !t->suppress && !t->multi && !t->moved)
      tap(t->sx, t->sy);
    t->down = 0;
    return 1;
  }
  return t->id[0] >= 0;
}
static int input_self_test(void) {
  power_self_test();
  struct drm_buf frame = {.pitch = W * 2, .map = calloc(W * H, 2)};
  assert(frame.map);
  page = MENU;
  render(&frame);
  int x = 0, y = 0;
  rotate_point(&x, &y);
  assert(x == 319 && y == 479);
  rotate_point(&x, &y);
  assert(x == 0 && y == 0);
  struct input_touch t = {.fd = -1,
                          .id = {-1, -1},
                          .ax = {.minimum = 0, .maximum = 319},
                          .ay = {.minimum = 0, .maximum = 479}};
  const struct input_event events[] = {
      {.type = EV_ABS, .code = ABS_MT_SLOT, .value = 0},
      {.type = EV_ABS, .code = ABS_MT_TRACKING_ID, .value = 1},
      {.type = EV_ABS, .code = ABS_MT_POSITION_X, .value = 69},
      {.type = EV_ABS, .code = ABS_MT_POSITION_Y, .value = 459},
      {.type = EV_SYN, .code = SYN_REPORT},
      {.type = EV_ABS, .code = ABS_MT_TRACKING_ID, .value = -1},
      {.type = EV_SYN, .code = SYN_REPORT}};
  for (size_t i = 0; i < sizeof(events) / sizeof(events[0]); i++) {
    struct input_event e = events[i];
    input_event(&t, &e);
  }
  assert(quitting == 1);
  quitting = 0;
  screen_awake = 0;
  for (size_t i = 0; i < 5; i++) {
    struct input_event e = events[i]; input_event(&t, &e);
  }
  screen_awake = 1; /* A finger held over wake must not activate Stock UI. */
  for (size_t i = 5; i < 7; i++) {
    struct input_event e = events[i]; input_event(&t, &e);
  }
  assert(!quitting);
  t.suppress = 1; /* Entire dark tap queued before the wake key was drained. */
  for (size_t i = 0; i < 7; i++) {
    struct input_event e = events[i]; input_event(&t, &e);
  }
  assert(!quitting);
  t.suppress = 0;
  for (size_t i = 0; i < 5; i++) {
    struct input_event e = events[i];
    input_event(&t, &e);
  }
  struct input_event move = {
      .type = EV_ABS, .code = ABS_MT_POSITION_X, .value = 109};
  input_event(&t, &move);
  struct input_event syn = {.type = EV_SYN, .code = SYN_REPORT};
  input_event(&t, &syn);
  for (size_t i = 5; i < 7; i++) {
    struct input_event e = events[i];
    input_event(&t, &e);
  }
  assert(!quitting);
  for (size_t i = 0; i < 5; i++) {
    struct input_event e = events[i];
    input_event(&t, &e);
  }
  struct input_event drop = {.type = EV_SYN, .code = SYN_DROPPED};
  input_event(&t, &drop);
  input_event(&t, &syn);
  for (size_t i = 5; i < 7; i++) {
    struct input_event e = events[i];
    input_event(&t, &e);
  }
  assert(!quitting);
  free(frame.map);
  puts("Power press/repeat/long/drop, dark-touch rejection and rotated MT-B tests passed");
  return 0;
}
static int panel(int heartbeat, int idle_seconds) {
  page = MENU;
  struct qpic_ctx drm;
  struct input_touch input;
  int result = 1;
  if (input_open(&input) < 0) {
    logline("touch open failed: %s", strerror(errno));
    return 1;
  }
  if (drm_init(&drm) < 0) {
    drm_destroy(&drm);
    close(input.fd);
    return 1;
  }
  int keyfd = power_open();
  if (keyfd < 0) {
    logline("power key open failed");
    drm_destroy(&drm); close(input.fd); return 1;
  }
  struct power_button key = {0};
  int on_level = settings_brightness, check_step = 0;
  if (on_level <= 0) on_level = 128;
  screen_awake = 1;
  long long started = monotonic_ms();
  long long last = monotonic_ms(), next = 0, beat = 0, contact = last;
  int first = 1, settling = 0;
  long long settle_at = 0;
  char reply[49152];
  while (!g_stop && !quitting) {
    long long now = monotonic_ms();
    /* Idle: back to the stock UI, or with the owner's choice the screen goes dark and DevUI
     * stays. An explicit --supervise time, as the recovery check uses, always ends. */
    if (now - last >= (long long)(idle_seconds ? idle_seconds : settings_idle) * 1000) {
      if (idle_seconds || !settings_sleep)
        break;
      if (screen_awake && screen_toggle(&on_level))
        goto end;
      last = now;
    }
    if (now >= next) {
      next = now + 2000;
      struct snapshot incoming;
      if (exchange("status\n", reply, sizeof(reply)) == 0) {
        contact = now;
        if (parse_snapshot(reply, &incoming)) {
          struct snapshot prev = data;
          prev.stamp = incoming.stamp;
          prev.uptime = incoming.uptime;
          int data_diff = memcmp(&prev, &incoming, sizeof(struct snapshot)) != 0 ||
                          (page == OVERVIEW && data.uptime / 60 != incoming.uptime / 60);
          data = incoming;
          int old_brightness = settings_brightness;
          int changed = update_settings(&data);
          if (changed) {
            on_level = settings_brightness;
            if (screen_awake && !first && old_brightness != on_level) brightness(on_level);
          }
          int old_page = page, old_dialog = dialog;
          unsigned long long old_pending = pending;
          track_status(changed);
          if (!changed && !data_diff && page == old_page && dialog == old_dialog && pending == old_pending)
            need_draw = 0;
        }
      }
    }
    /* Node pages read their lists from the agent's cached view. */
    if ((page == NODES || page == GROUP) && now >= page_fetch_at && screen_awake)
      refresh_page();
    if (now - contact > 15000)
      break;
    if (settling && now >= settle_at && !screen_awake) {
      settling = 0; (void)!write(heartbeat, "R", 1);
    }
    if (settling && now >= settle_at && screen_awake) need_draw = 1;
    if (need_draw && screen_awake) {
      struct drm_buf *frame = drm_draw_buf(&drm);
      render(frame);
      rotate_frame(frame);
      if (first || settling || memcmp(frame->map, drm.bufs[drm.cur_buf ^ 1].map,
                          (size_t)frame->pitch * H)) {
        if (drm_flip(&drm) < 0)
          goto end;
      }
      need_draw = 0;
      if (first) {
        // QPIC modeset can reset the backlight. Illuminate only after a real frame.
        brightness(on_level);
        if (readint(BL) != on_level) goto end;
        settling = 2;
        settle_at = now + 150;
        first = 0;
        logline("first frame committed; backlight=%d", on_level);
      } else if (settling && now >= settle_at) {
        // Two bounded commits after modeset; no touch is required to prime QPIC.
        brightness(on_level);
        if (readint(BL) != on_level) goto end;
        settling--;
        settle_at = now + 200;
        if (!settling) {
          (void)!write(heartbeat, "R", 1);
          logline("display startup settled without touch");
        }
      }
    }
    if (now >= beat) {
      (void)!write(heartbeat, "H", 1);
      beat = now + 1000;
    }
    if (getppid() == 1)
      break;
    if (power_check && check_step < 3 && now - started >= (check_step + 1) * 1000) {
      if (screen_toggle(&on_level)) goto end;
      input.multi = 1;
      check_step++;
    }
    struct pollfd p[2] = {{.fd = input.fd, .events = POLLIN},
                         {.fd = keyfd, .events = POLLIN}};
    if (poll(p, 2, 50) < 0 && errno != EINTR) goto end;
    if ((p[0].revents | p[1].revents) & (POLLERR | POLLHUP | POLLNVAL)) goto end;
    struct input_event ev;
    input.suppress = 0;
    while (read(keyfd, &ev, sizeof(ev)) == sizeof(ev)) {
      long long stamp = (long long)ev.time.tv_sec * 1000 + ev.time.tv_usec / 1000;
      if (power_event(&key, ev.type, ev.code, ev.value, stamp)) {
        if (screen_toggle(&on_level)) goto end;
        input.multi = 1;
        input.suppress = 1; /* Reject touch queued across a backlight transition. */
        last = monotonic_ms();
      }
    }
    while (read(input.fd, &ev, sizeof(ev)) == sizeof(ev)) {
      if (input_event(&input, &ev) && screen_awake)
        last = monotonic_ms();
    }
  }
  result = power_check && check_step != 3 ? 1 : 0;
end:
  close(keyfd);
  drm_destroy(&drm);
  close(input.fd);
  return result;
}
static int supervise(int seconds, int recovery) {
  umask(077);
  if (mkdir(RUNDIR, 0700) < 0 && errno != EEXIST)
    return 1;
  int lock = open(RUNDIR "/lock", O_CREAT | O_RDWR | O_CLOEXEC, 0600);
  if (lock < 0 || flock(lock, LOCK_EX | LOCK_NB) < 0)
    return 2;
  char buf[2048];
  snprintf(buf, sizeof(buf), "%d", getpid());
  putfile(RUNDIR "/owner", buf);
  signal(SIGINT, on_sig);
  signal(SIGTERM, on_sig);
  signal(SIGHUP, SIG_IGN);
  signal(SIGPIPE, SIG_IGN);
  if (recovery) {
    int rc = restore();
    unlink(RUNDIR "/owner");
    close(lock);
    return rc;
  }
  char *sync[] = {"ubus",          "call", "zwrt_topsw_daemon.sync",
                  "get_sync_info", "{}",   NULL};
  int bl = readint(BL), ui = processes("zte_topsw_devui", 0) > 0,
      touch = processes("mtdev2tuio", 0) > 0;
  if (run(sync, buf, sizeof(buf)) || !strstr(buf, "sync success") ||
      !strstr(buf, "register success") || !ui || bl < 0) {
    putfile(RUNDIR "/state", "error");
    unlink(RUNDIR "/owner");
    close(lock);
    return 1;
  }
  snprintf(buf, sizeof(buf), "%d", bl);
  putfile(RUNDIR "/brightness", buf);
  putfile(RUNDIR "/ui", ui ? "1" : "0");
  putfile(RUNDIR "/touch", touch ? "1" : "0");
  putfile(RUNDIR "/state", "starting");
  char *stop[] = {"/etc/init.d/zte_topsw_devui", "stop", NULL};
  int rc = 1;
  int stop_result = run(stop, NULL, 0);
  logline("stock stop result=%d", stop_result);
  if (stop_result)
    goto done;
  // Some firmware starts devui outside procd; stop only matching executable
  // paths.
  processes("zte_topsw_devui", SIGTERM);
  if (touch)
    processes("mtdev2tuio", SIGTERM);
  for (int i = 0; i < 30 && (processes("zte_topsw_devui", 0) ||
                             processes("mtdev2tuio", 0));
       i++)
    usleep(100000);
  processes("zte_topsw_devui", SIGKILL);
  if (touch)
    processes("mtdev2tuio", SIGKILL);
  usleep(200000);
  logline("after stop: ui=%d touch=%d signal=%d",
          processes("zte_topsw_devui", 0), processes("mtdev2tuio", 0),
          (int)g_stop);
  if (processes("zte_topsw_devui", 0) || processes("mtdev2tuio", 0) || g_stop)
    goto done;
  // Keep the handoff dark until the child has committed its first frame.
  brightness(0);
  int pipefd[2];
  if (pipe2(pipefd, O_CLOEXEC | O_NONBLOCK) < 0)
    goto done;
  pid_t child = fork();
  if (child < 0) {
    close(pipefd[0]);
    close(pipefd[1]);
    goto done;
  }
  if (!child) {
    close(pipefd[0]);
    close(lock);
    int r = panel(pipefd[1], seconds);
    close(pipefd[1]);
    if (getppid() == 1)
      restore();
    _exit(r);
  }
  close(pipefd[1]);
  snprintf(buf, sizeof(buf), "%d", child);
  putfile(RUNDIR "/child", buf);
  long long beat = monotonic_ms();
  int status = 0, exited = 0, ready = 0;
  while (!g_stop && monotonic_ms() - beat < 10000) {
    char bytes[64];
    ssize_t n = read(pipefd[0], bytes, sizeof(bytes));
    if (n > 0) {
      beat = monotonic_ms();
      for (ssize_t i = 0; i < n; i++)
        if (bytes[i] == 'R') {
          ready = 1;
          putfile(RUNDIR "/state", "active");
        }
    }
    if (waitpid(child, &status, WNOHANG) == child) {
      exited = 1;
      break;
    }
    usleep(100000);
  }
  if (!exited) {
    kill(child, SIGTERM);
    for (int i = 0; i < 20; i++) {
      if (waitpid(child, &status, WNOHANG) == child) {
        exited = 1;
        break;
      }
      usleep(100000);
    }
    if (!exited) {
      kill(child, SIGKILL);
      waitpid(child, &status, 0);
    }
  }
  rc = ready && WIFEXITED(status) && WEXITSTATUS(status) == 0 ? 0 : 1;
  close(pipefd[0]);
  unlink(RUNDIR "/child");
done:;
  int restored = restore();
  logline("restoration result=%d; panel result=%d", restored, rc);
  if (restored)
    rc = 1;
  if (rc)
    putfile(RUNDIR "/state", "error");
  unlink(RUNDIR "/owner");
  close(lock);
  return rc;
}
int main(int argc, char **argv) {
  if (argc == 2 && !strcmp(argv[1], "--self-test"))
    return input_self_test();
  if (argc == 2 && !strcmp(argv[1], "--version")) {
    puts("openui-screen 5 DevUI / live settings / four-press shortcut / proxy and Clash");
    return 0;
  }
  if (argc == 2 && !strcmp(argv[1], "--ipc-check")) {
    // Read-only integration probe; never prints names, addresses or credentials.
    char reply[49152];struct snapshot snap;
    for(int i=0;i<8;i++) {
      if(exchange("status\n",reply,sizeof(reply))==0&&parse_snapshot(reply,&snap)&&snap.stamp) {
        printf("Agent IPC: %d profiles, %d devices, overview=%s, proxy=%s\n",snap.np,snap.n,snap.uptime?"ready":"unavailable",snap.backend.has?"ready":"unavailable");
        return snap.uptime?0:1;
      }
      usleep(500000);
    }
    return 1;
  }
  if (argc == 2 && !strcmp(argv[1], "--power-check")) {
    power_check = 1;
    return supervise(5, 0);
  }
  if (argc == 2 && !strcmp(argv[1], "--probe")) {
    int fd = power_open();
    if (fd < 0) return 1;
    close(fd);
    puts("Power: pmic_pwrkey KEY_POWER, non-exclusive observer");
    struct input_touch t;
    if (input_open(&t) < 0)
      return 1;
    printf("Touch X=%d..%d Y=%d..%d; target 320x480 RGB565 Atomic\n",
           t.ax.minimum, t.ax.maximum, t.ay.minimum, t.ay.maximum);
    close(t.fd);
    return 0;
  }
  if (argc >= 2 && !strcmp(argv[1], "--supervise")) {
    int seconds = argc == 3 ? atoi(argv[2]) : 0;
    if (argc > 3 || (argc == 3 && (seconds < 3 || seconds > 600)))
      return 2;
    return supervise(seconds, 0);
  }
  if (argc == 2 && !strcmp(argv[1], "--restore"))
    return supervise(0, 1);
  fprintf(stderr, "Use --supervise [3..600 idle seconds] or --restore\n");
  return 2;
}
#else
static void dump(struct drm_buf *b, const char *path) {
  FILE *f = fopen(path, "wb");
  assert(f);
  fprintf(f, "P6\n320 480\n255\n");
  for (int i = 0; i < W * H; i++) {
    uint16_t c = b->map[i];
    unsigned char rgb[3] = {(unsigned char)(((c >> 11) & 31) * 255 / 31),
                            (unsigned char)(((c >> 5) & 63) * 255 / 63),
                            (unsigned char)((c & 31) * 255 / 31)};
    fwrite(rgb, 1, 3, f);
  }
  fclose(f);
}
static size_t html_peak;
static char html_peak_page[64];
/* Every page must fit the HTML buffer whole: a truncated document drops controls. */
static void capture(struct drm_buf *b,const char *dir,const char *name) {
  char path[512];render(b);
  assert(strstr(html,"</body></html>")&&html_used<sizeof(html)-512);
  if(html_used>html_peak){html_peak=html_used;snprintf(html_peak_page,sizeof(html_peak_page),"%s",name);}
  snprintf(path,sizeof(path),"%s/%s.ppm",dir,name);dump(b,path);
}
/* The same page in Chinese and English ("en-" prefix). */
static void capture_both(struct drm_buf *b,const char *dir,const char *name) {
  char en[64];int zh=settings_zh;
  settings_zh=1;capture(b,dir,name);
  settings_zh=0;snprintf(en,sizeof(en),"en-%s",name);capture(b,dir,en);
  settings_zh=zh;
}
static void press_id(struct drm_buf *b,const char *id) {
  render(b);
  char selector[48];snprintf(selector,sizeof(selector),"#%s",id);
  int x,y,w,h;
  assert(html_view_rect(selector,&x,&y,&w,&h));
  assert(w>=48&&h>=48&&x>=0&&y>=0&&x+w<=W&&y+h<=H);
  tap(x+w/2,y+h/2);
}
#define CJK48 "超长名称测试超长名称测试超长名称测试超长名称测试超长名称测试超长名称测试超长名称测试超长名称测试"
/* A status from an agent with proxy support; names are worst-case CJK. */
static void proxy_snapshot(char *out, size_t cap) {
  unsigned long long now=(unsigned long long)time(NULL);
  size_t n=(size_t)snprintf(out,cap,
    "S 7 1 0 0 0 0 134217728 25165824 1 %llu\n"
    "P\t1\t1\tHome VPN\nD\t02:11:22:33:44:55\t1\t1\t192.0.2.2\tWork laptop\n"
    "O 83 320 18372 34.5 12.4\n"
    "B\tclash\t0\t7\t1\t-\t0\t12\t1\t-\t0\n"
    "K\t12\t1\t1\t1\trule\t3600\t43008\t3\t1\tv1.19.13\t-\n"
    "U\t3\t1\t1\t%llu\t1073741824\t5368709120\t107374182400\t%llu\t42\t0\t\xF0\x9F\x87\xAD\xF0\x9F\x87\xB0 香港 Premium\n"
    "U\t4\t0\t1\t%llu\t0\t2147483648\t0\t0\t17\t0\t" CJK48 "\n"
    "U\t5\t0\t0\t0\t0\t0\t0\t0\t0\t0\t" CJK48 "\n"
    "U\t6\t0\t1\t%llu\t1\t1\t1073741824\t%llu\t9\t1\t" CJK48 "\n"
    "U\t7\t0\t1\t%llu\t0\t0\t1073741824\t%llu\t9\t0\t" CJK48 "\n"
    "M\t1\tv1.19.13\tpinned\tv1.19.12\tv1.19.14\t2026-09-30\t1\tupdate\trunning\tdownloading\t5242880\t31457280\t209715200\t-\n"
    "X\t0\t%llu\tnodes\n"
    "x\tclock\tpass\t3\t1\t-\t-\nx\tcore\tpass\t1\t1\t-\t-\nx\tdns\tpass\t1\t0\t-\t-\n"
    "x\tnodes\tfail\t-\t-\t0\t24\nx\tfetch\tfail\t200\t-\t-\t-\nx\tdevices\twarn\t0\t5\t-\t-\n"
    "x\troute\tskip\t-\t-\t-\t-\nx\tfirewall\twarn\t0\t1\t0\t12\n"
    "C 9 120 128 2 27 zh\nV 9 120 128 2 113 zh\nJ 0 3 3 0 3\n.\n",
    now,now-7200,now+30*86400,now-86400*3,now-60,now+3*86400,now-600,now-86400,now-120);
  assert(n<cap);
}
static const char groups_page1[]=
  "G\t1\t4242\t9\t0\n"
  "g\t0\t1\t6\t5\tSelector\t\xF0\x9F\x9A\x80 节点选择\t\xF0\x9F\x87\xAD\xF0\x9F\x87\xB0 香港 01\n"
  "g\t1\t0\t12\t0\tURLTest\t\xE2\x99\xBB\xEF\xB8\x8F 自动选择\t\xF0\x9F\x87\xAF\xF0\x9F\x87\xB5 东京 02\n"
  "g\t2\t1\t3\t0\tSelector\tStreaming\tDIRECT\n"
  "g\t3\t1\t2\t1\tFallback\t" CJK48 "\t" CJK48 "\n.\n";
static const char groups_page2[]=
  "G\t1\t4242\t9\t4\ng\t4\t1\t2\t0\tSelector\tGLOBAL\tDIRECT\n.\n";
static const char nodes_page2[]=
  "N\t4242\t0\t6\t4\t1\t5\tSelector\t\xF0\x9F\x9A\x80 节点选择\n"
  "n\t4\t88\tTrojan\t\xF0\x9F\x87\xAD\xF0\x9F\x87\xB0 香港 01 " CJK48 "\n"
  "n\t5\t0\tVmess\t\xF0\x9F\x87\xAF\xF0\x9F\x87\xB5 东京 02\n.\n";
static const char nodes_page1[]=
  "N\t4242\t0\t6\t0\t1\t5\tSelector\t\xF0\x9F\x9A\x80 节点选择\n"
  "n\t0\t-1\tURLTest\t\xE2\x99\xBB\xEF\xB8\x8F 自动选择\n"
  "n\t1\t1234\tShadowsocks\t" CJK48 "\n"
  "n\t2\t156\tVless\t\xF0\x9F\x87\xBA\xF0\x9F\x87\xB8 United States 03\n"
  "n\t3\t420\tHysteria2\t\xF0\x9F\x87\xB8\xF0\x9F\x87\xAC 新加坡 04\n.\n";
static const char nodes_auto[]=
  "N\t4242\t1\t2\t0\t0\t0\tURLTest\t\xE2\x99\xBB\xEF\xB8\x8F 自动选择\n"
  "n\t0\t88\tTrojan\tHK 01\nn\t1\t0\tVmess\tJP 02\n.\n";
static void expect_request(const char *want) {
  if(strcmp(test_request,want)){fprintf(stderr,"request %s, want %s",test_request,want);assert(0);}
}
static void reset_job(void) { pending=0;pending_kind=0;dialog=0;notice[0]=0;test_request[0]=0; }
/* Every character whole: no sequence cut short. */
static int utf8_whole(const char *s) {
  for(const unsigned char *p=(const unsigned char *)s;*p;) {
    int n=*p<0x80?1:(*p&0xe0)==0xc0?2:(*p&0xf0)==0xe0?3:(*p&0xf8)==0xf0?4:0;
    if(!n)return 0;
    for(int i=1;i<n;i++)if((p[i]&0xc0)!=0x80)return 0;
    p+=n;
  }
  return 1;
}
static void proxy_tests(struct drm_buf *b,const char *dir) {
  char text[512],full[16384];
  /* Display text: flags become letters; other non-BMP characters, selectors and broken UTF-8 go. */
  const char *cases[][2]={
    {"\xF0\x9F\x87\xAD\xF0\x9F\x87\xB0 香港 01","HK 香港 01"},
    {"\xF0\x9F\x9A\x80 节点选择","节点选择"},
    {"\xE2\x99\xBB\xEF\xB8\x8F 自动选择","\xE2\x99\xBB 自动选择"},
    {"\xF0\x9F\x87\xAF\xF0\x9F\x87\xB5Tokyo","JPTokyo"},
    {"  a  \xF0\x9F\x8E\x89  b  ","a b"},
    {"\xFF\xFE""A\x80""B","AB"},
    {"cut \xE4\xB8","cut"},
    {"\xF0\x9F\x8E\x89","",}};
  for(size_t i=0;i<sizeof(cases)/sizeof(cases[0]);i++) {
    snprintf(text,sizeof(text),"%s",cases[i][0]);display_text(text);
    if(strcmp(text,cases[i][1])){fprintf(stderr,"display_text(%s)=%s\n",cases[i][0],text);assert(0);}
  }
  /* A field longer than its buffer ends before the first character that does not fit. */
  char small[8];
  copy_field(small,sizeof(small),"abc机器人");assert(!strcmp(small,"abc机"));
  copy_field(small,sizeof(small),"abcd机器");assert(!strcmp(small,"abcd机"));
  copy_field(small,sizeof(small),"abcde机");assert(!strcmp(small,"abcde"));
  copy_field(small,sizeof(small),"abcdefghij");assert(!strcmp(small,"abcdefg"));
  copy_field(small,sizeof(small),"-");assert(!small[0]);
  struct snapshot cut;
  char long_text[1024];
  snprintf(long_text,sizeof(long_text),"V 1 90 200 2 113 zh\nK\t1\t1\t0\t0\trule\t0\t0\t0\t1\tv1-%s\ta%s\nE\t4\t%s\n.\n",
           "版版版版版版版版版版版版","错错错错错错错错错错错错错错错错错错错错错错错错错错错错错错错错错","失败失败失败失败失败失败失败失败失败失败失败失败失败失败失败失败失败");
  assert(parse_snapshot(long_text,&cut)&&cut.clash.has&&cut.failed_job==4);
  assert(!strcmp(cut.clash.version,"v1-版版版版版版版版版")&&strlen(cut.clash.error)==94);
  assert(utf8_whole(cut.clash.version)&&utf8_whole(cut.clash.error)&&utf8_whole(cut.failure)&&strlen(cut.failure)==96);
  /* Settings: V wins over C in either order; an invalid V falls back to C; C alone is an
   * agent before Proxy and Clash, whose screen has neither page. */
  struct snapshot prefs;
  char both[]="V 20 90 200 5 113 en\nC 20 90 200 2 17 en\n.\n";
  assert(parse_snapshot(both,&prefs)&&update_settings(&prefs)&&settings_menu==113&&settings_home==PROXY&&!settings_zh);
  char bad_v[]="C 21 90 200 4 17 zh\nV 21 90 200 4 145 zh\n.\n";
  assert(parse_snapshot(bad_v,&prefs)&&update_settings(&prefs)&&settings_menu==17&&settings_home==OVERVIEW&&settings_zh);
  char hidden_home[]="C 22 90 200 2 17 zh\nV 22 90 200 6 49 zh\n.\n";
  assert(parse_snapshot(hidden_home,&prefs)&&update_settings(&prefs)&&settings_menu==17&&settings_home==MENU);
  char old_agent[]="C 23 90 200 2 27 zh\n.\n";
  assert(parse_snapshot(old_agent,&prefs)&&update_settings(&prefs)&&settings_menu==27);
  assert(!visible_page(PROXY)&&!visible_page(CLASH)&&!reachable(NODES)&&!reachable(DIAG));
  page=MENU;capture(b,dir,"menu-old-agent");assert(!strstr(html,"act:proxy")&&!strstr(html,"act:clash"));
  /* The new default; profiles and devices stay reachable through WireGuard. */
  proxy_snapshot(full,sizeof(full));
  assert(parse_snapshot(full,&data)&&update_settings(&data)==1&&settings_menu==113);
  assert(data.backend.has&&data.backend.active==BK_CLASH&&data.backend.rev[0]==7&&data.backend.rev[1]==12);
  assert(data.clash.has&&data.clash.rev==12&&data.clash.running&&!strcmp(data.clash.version,"v1.19.13")&&!data.clash.error[0]);
  assert(data.nsubs==5&&!strcmp(data.subs[0].name,"HK 香港 Premium")&&data.subs[3].failed&&!data.subs[2].downloaded);
  assert(data.core.available&&data.core.newer&&!strcmp(data.core.latest,"v1.19.14")&&!strcmp(data.core.state,"running"));
  assert(data.diag.has&&data.diag.n==8&&data.diag.checks[3].v[0]==NO_FIGURE&&data.diag.checks[3].v[3]==24);
  assert(data.has_fresh&&data.fresh==3&&data.n==1&&data.np==1);
  assert(!visible_page(PROFILES)&&reachable(PROFILES)&&reachable(DEVICES)&&reachable(DIAG));
  page=MENU;capture_both(b,dir,"menu-proxy");
  assert(strstr(html,"act:proxy")&&strstr(html,"act:clash")&&!strstr(html,"act:profiles"));
  page=WG;render(b);assert(strstr(html,"act:profiles")&&strstr(html,"act:devices"));
  /* Proxy page: confirmation before every switch, with the right revision. */
  page=MENU;press_id(b,"proxy");assert(page==PROXY);capture_both(b,dir,"proxy");
  press_id(b,"bk2");assert(!dialog); /* the active backend */
  press_id(b,"bk1");assert(dialog==5&&confirm_what==CF_BACKEND&&confirm_target==BK_WG);
  capture_both(b,dir,"proxy-confirm");
  press_id(b,"dismiss");assert(!dialog&&!test_request[0]);
  press_id(b,"bk1");press_id(b,"confirm");expect_request("backend 7 wireguard\n");
  assert(pending==1&&pending_kind==JOB_BACKEND&&!dialog);
  capture(b,dir,"proxy-switching");press_id(b,"bk0");assert(!dialog);
  reset_job();press_id(b,"bk0");assert(dialog==5);capture_both(b,dir,"proxy-off-confirm");
  /* Off names the backend shown as on, which the agent checks is still the one on. */
  press_id(b,"confirm");expect_request("backend 12 off clash\n");
  reset_job();data.backend.active=BK_WG;data.backend.ready[1]=0;snprintf(data.backend.reason[1],sizeof(data.backend.reason[1]),"sub");
  capture_both(b,dir,"proxy-not-ready");assert(strstr(html,"Add a subscription in OpenUI"));
  press_id(b,"bk2");assert(!dialog);
  data.backend.ready[1]=1;press_id(b,"bk2");press_id(b,"confirm");expect_request("backend 12 clash\n");
  reset_job();data.backend.active=BK_OFF;data.backend.conflict=0;capture(b,dir,"proxy-off");
  data.backend.active=BK_WG;data.backend.conflict=1;data.enabled=1;capture(b,dir,"proxy-conflict");
  /* Both saved on: choosing WireGuard is how the conflict is resolved. */
  press_id(b,"bk1");assert(dialog==5&&confirm_target==BK_WG);press_id(b,"dismiss");
  press_id(b,"bk0");assert(dialog==5&&confirm_target==BK_OFF);press_id(b,"confirm");expect_request("backend 7 off wireguard\n");
  reset_job();
  press_id(b,"active");assert(page==WG);
  /* WireGuard switches through the backend verb; toggle only for agents without it. */
  data.backend.conflict=0;press_id(b,"toggle");assert(dialog==1);press_id(b,"confirm");expect_request("backend 7 off wireguard\n");
  reset_job();data.enabled=0;data.backend.active=BK_CLASH;capture(b,dir,"wireguard-from-clash");
  press_id(b,"toggle");assert(dialog==5&&confirm_target==BK_WG);press_id(b,"confirm");expect_request("backend 7 wireguard\n");
  reset_job();data.backend.active=BK_OFF;press_id(b,"toggle");expect_request("backend 7 wireguard\n");assert(!dialog);
  reset_job();data.backend.active=BK_CLASH;
  /* Clash page: modes switch at once, turning off asks first. */
  page=MENU;press_id(b,"clash");assert(page==CLASH);capture_both(b,dir,"clash");
  press_id(b,"m0");assert(!test_request[0]);
  press_id(b,"m1");expect_request("cmode 12 global\n");assert(pending_kind==JOB_MODE);
  reset_job();press_id(b,"ctoggle");assert(dialog==5&&confirm_target==BK_OFF);press_id(b,"confirm");expect_request("backend 12 off clash\n");
  reset_job();data.clash.enabled=0;data.clash.running=0;data.backend.active=BK_WG;capture(b,dir,"clash-off");
  press_id(b,"ctoggle");assert(dialog==5&&confirm_target==BK_CLASH);press_id(b,"confirm");expect_request("backend 12 clash\n");
  reset_job();data.backend.active=BK_OFF;press_id(b,"ctoggle");expect_request("backend 12 clash\n");assert(!dialog);
  reset_job();data.backend.active=BK_CLASH;data.clash.enabled=1;data.clash.running=1;
  snprintf(data.clash.error,sizeof(data.clash.error),"Clash policy is incomplete; automatic recovery is pending");
  capture(b,dir,"clash-error");data.clash.error[0]=0;
  /* Nodes: a page of groups, then a page of a group's nodes, by index and generation. */
  stub_groups=groups_page1;test_log[0]=0;
  press_id(b,"nodes");assert(page==NODES&&groups.state==1&&groups.n==4&&groups.gen==4242);
  expect_request("groups 0\n");
  assert(!strcmp(groups.rows[0].name,"节点选择")&&!strcmp(groups.rows[0].now_name,"HK 香港 01"));
  capture_both(b,dir,"nodes");
  stub_groups=groups_page2;press_id(b,"next");expect_request("groups 4\n");assert(group_offset==4&&groups.n==1);
  capture(b,dir,"nodes-page2");
  stub_groups=groups_page1;press_id(b,"prev");expect_request("groups 0\n");assert(group_offset==0);
  stub_nodes=nodes_page2;press_id(b,"g0");assert(page==GROUP&&open_gen==4242&&open_group==0);
  expect_request("nodes 4242 0 4\n");  /* opens on the page of the current node */
  assert(nodes.ok&&node_offset==4&&nodes.n==2&&nodes.now==5&&!strncmp(nodes.rows[0].name,"HK 香港 01 ",13));
  capture_both(b,dir,"group");
  stub_nodes=nodes_page1;press_id(b,"prev");expect_request("nodes 4242 0 0\n");assert(node_offset==0&&nodes.n==4);
  capture_both(b,dir,"group-page1");
  press_id(b,"n2");expect_request("select 4242 0 2\n");assert(pending==1&&pending_kind==JOB_SELECT);
  capture(b,dir,"group-selecting");
  reset_job();press_id(b,"test");expect_request("delay 4242 0\n");assert(pending_kind==JOB_DELAY);
  capture(b,dir,"group-testing");
  /* Completion waits until the records show the job, then reloads the nodes. */
  data.busy=0;data.done=1;data.fresh=0;data.result=0;data.has_fresh=1;settle_jobs();assert(pending==1);
  data.fresh=1;settle_jobs();assert(!pending&&!pending_kind);expect_request("nodes 4242 0 0\n");
  /* A stale generation reloads the group list. */
  stub_job="ERR Stale\n";test_log[0]=0;press_id(b,"n3");
  assert(page==NODES&&!pending&&notice[0]&&strstr(test_log,"select 4242 0 3\n")&&strstr(test_log,"groups 0\n"));
  capture(b,dir,"nodes-stale");stub_job="OK 1\n";
  page=GROUP;stub_nodes="ERR Stale\n";refresh_page();assert(page==NODES);
  /* ...but not under the failure dialog: its message and the page stay until it closes. */
  page=GROUP;dialog=3;snprintf(notice,sizeof(notice),"%s",job_failure(JOB_SELECT,2));refresh_page();
  assert(page==GROUP&&dialog==3&&!strcmp(notice,job_failure(JOB_SELECT,2)));
  press_id(b,"refresh");assert(page==NODES&&!dialog&&strstr(notice,"节点列表已变化"));
  stub_nodes=nodes_auto;notice[0]=0;press_id(b,"g1");assert(page==GROUP&&!nodes.selectable);
  capture(b,dir,"group-auto");test_request[0]=0;press_id(b,"n1");assert(!test_request[0]&&!pending);
  press_id(b,"back");assert(page==NODES);press_id(b,"back");assert(page==CLASH);
  stub_groups="G\t2\t0\t0\t0\n.\n";press_id(b,"nodes");assert(groups.state==2);capture(b,dir,"nodes-not-running");
  press_id(b,"back");
  /* Subscriptions: a dialog to make one current or update it now. */
  press_id(b,"subs");assert(page==SUBS);capture_both(b,dir,"subs");
  press_id(b,"s1");assert(dialog==6&&chosen_sub==4&&chosen_sub_rev==12);capture_both(b,dir,"sub-dialog");
  press_id(b,"activate");expect_request("csub 12 4\n");assert(pending_kind==JOB_SUB&&!dialog);
  reset_job();press_id(b,"s1");press_id(b,"srefresh");expect_request("crefresh 4\n");assert(pending_kind==JOB_REFRESH);
  reset_job();press_id(b,"s0");test_request[0]=0;press_id(b,"activate");assert(!test_request[0]&&dialog==6);
  capture(b,dir,"sub-dialog-active");press_id(b,"dismiss");assert(!dialog);
  press_id(b,"back");assert(page==CLASH);
  /* Core and data: no action while a core job runs; update and rollback confirm. */
  press_id(b,"core");assert(page==CORE);capture_both(b,dir,"core");
  press_id(b,"ccheck");assert(!test_request[0]);
  snprintf(data.core.state,sizeof(data.core.state),"done");
  press_id(b,"ccheck");expect_request("core check\n");assert(pending_kind==JOB_CORE);
  reset_job();press_id(b,"cupdate");assert(dialog==5&&confirm_what==CF_UPDATE);capture_both(b,dir,"core-confirm");
  press_id(b,"confirm");expect_request("core update v1.19.14\n");
  reset_job();press_id(b,"crollback");assert(dialog==5);press_id(b,"confirm");expect_request("core rollback\n");
  reset_job();press_id(b,"cgeo");assert(dialog==5);press_id(b,"confirm");expect_request("core geodata\n");
  reset_job();snprintf(data.core.latest,sizeof(data.core.latest),"v1 2");press_id(b,"cupdate");assert(!dialog);
  /* A tag the agent would refuse draws "Update core" disabled, as tapping it does nothing. */
  render(b);assert(strstr(html,"id=\"cupdate\" href=\"act:none\""));
  snprintf(data.core.latest,sizeof(data.core.latest),"v1+build.5");render(b);
  assert(data.core.newer&&core_idle()&&!core_update_ok()&&strstr(html,"id=\"cupdate\" href=\"act:none\""));
  press_id(b,"cupdate");assert(!dialog);
  snprintf(data.core.latest,sizeof(data.core.latest),"v1.19.14");render(b);assert(strstr(html,"id=\"cupdate\" href=\"act:cupdate\""));
  snprintf(data.core.state,sizeof(data.core.state),"failed");snprintf(data.core.error,sizeof(data.core.error),"Download failed: timeout");
  capture(b,dir,"core-failed");
  /* A refused job explains itself with the agent's message. */
  send_job("core check\n",JOB_CORE,0);data.done=pending;data.fresh=pending;data.result=1;data.failed_job=pending;
  snprintf(data.failure,sizeof(data.failure),"A core job is already running");
  settle_jobs();assert(dialog==3&&!pending&&!strcmp(failure_text,"A core job is already running"));
  capture_both(b,dir,"job-failed");press_id(b,"refresh");assert(!dialog&&page==CORE&&!failure_text[0]);
  data.result=0;
  struct core_info saved_core=data.core;memset(&data.core,0,sizeof(data.core));data.core.has=1;
  test_request[0]=0;capture(b,dir,"core-unavailable");press_id(b,"ccheck");assert(!test_request[0]);data.core=saved_core;
  press_id(b,"back");press_id(b,"back");assert(page==MENU);
  /* Diagnostics: two pages of four checks, reached from the proxy page. */
  press_id(b,"proxy");press_id(b,"diag");assert(page==DIAG&&diag_from==PROXY);capture_both(b,dir,"diag");
  press_id(b,"next");assert(diag_offset==4);capture_both(b,dir,"diag-page2");
  press_id(b,"prev");assert(!diag_offset);
  /* B31's Beijing-time system clock with the core's NTP passes and says why. */
  struct check_info saved_clock=data.diag.checks[0];
  assert(!strcmp(data.diag.checks[0].id,"clock"));
  data.diag.checks[0].v[0]=28796;data.diag.checks[0].v[2]=1;
  capture(b,dir,"diag-beijing");assert(strstr(html,"北京时间，快 8 小时，内核已校正"));
  settings_zh=0;capture(b,dir,"en-diag-beijing");assert(strstr(html,"Beijing time +8 h; core uses NTP"));settings_zh=1;
  data.diag.checks[0]=saved_clock;
  press_id(b,"drun");expect_request("diag\n");assert(pending_kind==JOB_DIAG);
  reset_job();data.diag.running=1;capture(b,dir,"diag-running");press_id(b,"drun");assert(!test_request[0]);
  data.diag.running=0;press_id(b,"back");assert(page==PROXY);press_id(b,"back");assert(page==MENU);
  /* Diagnostics from the Clash page, with Proxy hidden: Overview, WireGuard and Clash. */
  char ow_clash[]="C 24 90 200 2 27 zh\nV 24 90 200 2 81 zh\n.\n";
  assert(parse_snapshot(ow_clash,&prefs)&&update_settings(&prefs)==1&&settings_menu==81);
  assert(!visible_page(PROXY)&&reachable(DIAG));
  page=MENU;press_id(b,"clash");assert(page==CLASH);
  press_id(b,"diag");assert(page==DIAG&&diag_from==CLASH);press_id(b,"back");assert(page==CLASH);
  /* A shorter check list (a new run clears it) brings the page back to one it has. */
  press_id(b,"diag");press_id(b,"next");assert(diag_offset==4);
  data.diag.n=3;track_status(0);assert(!diag_offset);
  render(b);assert(strstr(html,">1 / 1<")&&strstr(html,"id=\"prev\" href=\"act:none\""));
  diag_offset=4;data.diag.n=0;track_status(0);assert(!diag_offset);data.diag.n=8;
  press_id(b,"back");assert(page==CLASH);
  data.backend.ready[1]=0;data.clash.enabled=0;data.clash.running=0;data.backend.active=BK_OFF;
  snprintf(data.backend.reason[1],sizeof(data.backend.reason[1]),"download");
  capture_both(b,dir,"clash-not-ready");assert(strstr(html,"Update the subscription first"));
  data.backend.ready[1]=1;data.clash.enabled=1;data.clash.running=1;data.backend.active=BK_CLASH;
  press_id(b,"back");assert(page==MENU);
  char all_four[]="C 25 120 128 2 27 zh\nV 25 120 128 2 113 zh\n.\n";
  assert(parse_snapshot(all_four,&prefs)&&update_settings(&prefs)==1&&settings_menu==113);
  /* Busy and failure replies keep the panel usable. */
  stub_job="ERR Busy\n";page=CORE;press_id(b,"ccheck");assert(dialog==3&&!pending&&strstr(notice,"仍在进行"));
  stub_job="OK 1\n";reset_job();
  /* Agents before `fresh`: a WireGuard job is done once the revision rises. */
  pending=3;pending_kind=JOB_WG;pending_revision=7;data.has_fresh=0;data.job=3;data.done=3;data.result=0;data.rev=7;
  pending_deadline=monotonic_ms()+60000;
  settle_jobs();assert(pending==3);data.rev=8;settle_jobs();assert(!pending);
  /* A restarted agent no longer knows the job: the panel stops waiting for it. */
  pending=4;pending_kind=JOB_BACKEND;data.job=0;data.done=0;settle_jobs();assert(!pending&&notice[0]);notice[0]=0;
  /* An outcome that never comes: the panel waits a bound, then says so. */
  assert(job_wait_ms(JOB_BACKEND)==180000&&job_wait_ms(JOB_CORE)==180000&&job_wait_ms(JOB_REFRESH)==180000);
  assert(job_wait_ms(JOB_SELECT)==60000&&job_wait_ms(JOB_DELAY)==60000&&job_wait_ms(JOB_DIAG)==60000);
  reset_job();data.has_fresh=1;data.result=0;page=CLASH;press_id(b,"m2");
  expect_request("cmode 12 direct\n");assert(pending==1&&pending_kind==JOB_MODE);
  long long wait=pending_deadline-monotonic_ms();assert(wait>170000&&wait<=180000);
  data.job=1;data.done=0;data.fresh=0;settle_jobs();assert(pending==1&&!dialog);
  pending_deadline=monotonic_ms()-1;settle_jobs();assert(!pending&&!pending_kind&&dialog==3&&strstr(notice,"未及时"));
  capture(b,dir,"job-timeout");press_id(b,"refresh");assert(!dialog&&page==CLASH);
  settings_zh=0;press_id(b,"m2");pending_deadline=monotonic_ms()-1;settle_jobs();
  assert(dialog==3&&strstr(notice,"in time"));capture(b,dir,"en-job-timeout");settings_zh=1;reset_job();
  /* So does an old agent whose revision never rises. */
  pending=3;pending_kind=JOB_WG;pending_revision=7;data.has_fresh=0;data.job=3;data.done=3;data.rev=7;
  pending_deadline=monotonic_ms()-1;settle_jobs();assert(!pending&&dialog==3);reset_job();
  /* A Clash-only job does not make WireGuard look busy, nor a WireGuard one Clash. */
  data.busy=0;pending=9;pending_kind=JOB_REFRESH;
  assert(applying(1)&&!applying(0)&&strcmp(summary(),L("Applying changes","正在应用更改")));
  assert(!strcmp(clash_summary(),L("Applying changes","正在应用更改")));
  pending_kind=JOB_SUB;assert(applying(1)&&!applying(0));pending_kind=JOB_MODE;assert(applying(1)&&!applying(0));
  pending_kind=JOB_WG;assert(applying(0)&&!applying(1)&&strcmp(clash_summary(),L("Applying changes","正在应用更改")));
  pending_kind=JOB_BACKEND;assert(applying(0)&&applying(1));
  pending_kind=JOB_DELAY;assert(!applying(0)&&!applying(1));
  reset_job();data.busy=1;assert(applying(0)&&applying(1));data.busy=0;
  /* Without WireGuard in the menu, its profile and device pages return to the menu. */
  char no_wg[]="C 26 90 200 2 26 zh\nV 26 90 200 2 26 zh\n.\n";
  assert(parse_snapshot(no_wg,&prefs)&&update_settings(&prefs)==1&&!reachable(WG)&&reachable(DEVICES));
  data.stamp=(unsigned long long)time(NULL);
  page=MENU;press_id(b,"devices");assert(page==DEVICES);press_id(b,"d0");assert(dirty);
  press_id(b,"back");assert(dialog==2);press_id(b,"discard");assert(page==MENU&&!dirty&&!dialog);
  press_id(b,"devices");press_id(b,"d0");press_id(b,"back");press_id(b,"saveback");assert(pending==1&&return_after_save);
  data.job=data.done=data.fresh=1;data.has_fresh=1;data.result=0;settle_jobs();assert(!pending&&page==MENU);
  page=PROFILES;dialog=3;press_id(b,"refresh");assert(page==MENU&&!dialog);
  enter_devices();dialog=3;press_id(b,"refresh");assert(page==MENU&&!dialog);
  enter_devices();draft.n=0;submit(0,0);assert(page==MENU&&!pending);
  char all_four_again[]="C 27 120 128 2 27 zh\nV 27 120 128 2 113 zh\n.\n";
  assert(parse_snapshot(all_four_again,&prefs)&&update_settings(&prefs)==1&&settings_menu==113);
  /* Hiding Clash leaves its pages once nothing is pending. */
  char no_clash[]="C 30 90 200 2 17 zh\nV 30 90 200 2 17 zh\n.\n";
  page=NODES;assert(parse_snapshot(no_clash,&prefs)&&update_settings(&prefs)==1);
  track_status(0);assert(page==MENU);
  /* A full status with 128 CJK device names stays within the reply the panel reads. */
  size_t used=(size_t)snprintf(full,sizeof(full),"S 7 1 1 1 0 0 0 0 1 %llu\n",(unsigned long long)time(NULL));
  static char big[49152];
  memcpy(big,full,used);
  for(int i=0;i<128;i++)used+=(size_t)snprintf(big+used,sizeof(big)-used,"D\t02:11:22:33:%02x:%02x\t1\t1\t192.0.2.%d\t" CJK48 "\n",i/256,i%256,i);
  used+=(size_t)snprintf(big+used,sizeof(big)-used,"V 31 90 200 2 113 zh\nJ 0 0 0 0 0\n.\n");
  assert(used<40*1024&&parse_snapshot(big,&data)&&data.n==128);
  update_settings(&data);enter_devices();capture(b,dir,"devices-cjk");
  printf("PASS proxy, Clash, nodes, subscriptions, core and diagnostics pages (zh/en), switch requests, stale generations, job completion, flags and settings records; largest page %zu bytes (%s), status %zu bytes\n",
         html_peak,html_peak_page,used);
}
/* The eSIM page: W settings, Q/q records, switching, restart, paging and refusals. */
static void esim_tests(struct drm_buf *b,const char *dir) {
  char text[4096];
  struct snapshot s;
  unsigned long long now=(unsigned long long)time(NULL);
  snprintf(text,sizeof(text),
    "S 7 1 0 0 0 0 0 0 1 %llu\nO 83 320 18372 34.5 12.4\n"
    "Q\t1\teuicc\t42\t5\t0\t-\t-\t-\t-\t-\n"
    "q\t0\t1\toperational\t\xF0\x9F\x87\xAD\xF0\x9F\x87\xB0 香港 Hong Kong\t8901\n"
    "q\t1\t0\toperational\tTruphone\t4567\n"
    "q\t2\t0\ttest\t" CJK48 "\t0003\n"
    "q\t3\t0\toperational\tTravel\t0004\n"
    "q\t4\t0\tprovisioning\tBootstrap\t0005\n"
    "C 40 90 200 2 27 zh\nV 40 90 200 2 80 zh\nW 40 90 200 2 208 zh\nJ 0 5 5 0 5\n.\n",now);
  assert(parse_snapshot(text,&s)&&s.esim.has&&s.esim.installed&&s.esim.gen==42&&s.esim.total==5&&s.esim.n==5);
  assert(!strcmp(s.esim.card,"euicc")&&s.esim.rows[0].enabled&&!strcmp(s.esim.rows[0].name,"HK 香港 Hong Kong"));
  assert(!strcmp(s.esim.rows[1].tail,"4567")&&!s.esim.kind[0]&&s.esim.firmware_ready==-1&&!s.esim.reboot);
  data=s;assert(update_settings(&data)&&settings_menu==208&&visible_page(ESIM)&&visible_page(CLASH)&&!visible_page(PROXY));
  /* W wins over V; without W the V record, which has no eSIM, applies. */
  char no_w[]="C 41 90 200 2 27 zh\nV 41 90 200 2 80 zh\n.\n",bad_w[]="V 42 90 200 2 80 zh\nW 42 90 200 2 8396 zh\n.\n";
  struct snapshot prefs;
  assert(parse_snapshot(no_w,&prefs)&&update_settings(&prefs)&&settings_menu==80&&!visible_page(ESIM));
  assert(parse_snapshot(bad_w,&prefs)&&update_settings(&prefs)&&settings_menu==80);
  char home_w[]="V 43 90 200 2 80 en\nW 43 90 200 7 208 en\n.\n";
  assert(parse_snapshot(home_w,&prefs)&&update_settings(&prefs)&&settings_home==ESIM&&!settings_zh);
  /* The seventh W field is the idle action; a W record without it returns to the stock UI. */
  assert(!settings_sleep);
  char sleep_w[]="V 44 90 200 2 80 zh\nW 44 90 200 2 208 zh 1\n.\n",bad_sleep[]="V 45 90 200 2 80 zh\nW 45 90 200 2 208 zh 2\n.\n";
  assert(parse_snapshot(sleep_w,&prefs)&&update_settings(&prefs)&&settings_sleep&&settings_zh&&settings_menu==208);
  page=MENU;capture(b,dir,"menu-sleep");assert(strstr(html,"空闲 90 秒后熄屏"));
  settings_zh=0;capture(b,dir,"en-menu-sleep");assert(strstr(html,"Screen turns off after 90 s idle."));settings_zh=1;
  assert(parse_snapshot(bad_sleep,&prefs)&&update_settings(&prefs)&&!settings_sleep&&settings_menu==80);
  settings_zh=1;settings_menu=208;
  page=MENU;capture_both(b,dir,"menu-esim");assert(strstr(html,"act:esim")&&strstr(html,"Hong Kong"));
  test_request[0]=0;press_id(b,"esim");assert(page==ESIM&&!test_request[0]);capture_both(b,dir,"esim");
  /* The enabled profile is not tappable; another asks first, then names it by index. */
  press_id(b,"e0");assert(!dialog);
  press_id(b,"e1");assert(dialog==5&&confirm_what==CF_ESIM&&confirm_rev==42&&!strcmp(chosen_name,"Truphone"));
  capture_both(b,dir,"esim-confirm");
  press_id(b,"confirm");expect_request("esim enable 42 1\n");assert(pending_kind==JOB_ESIM&&!dialog);
  reset_job();
  press_id(b,"next");assert(esim_offset==4);capture(b,dir,"esim-page2");
  press_id(b,"e0");assert(dialog==5);press_id(b,"confirm");expect_request("esim enable 42 4\n");reset_job();
  press_id(b,"prev");assert(!esim_offset);
  /* A shorter list (a profile deleted on the webpage) keeps the page on one it has. */
  esim_offset=4;data.esim.n=4;track_status(0);assert(!esim_offset);data.esim.n=5;
  /* A list read again meanwhile: the agent refuses the old generation. */
  stub_job="ERR Stale\n";press_id(b,"e3");press_id(b,"confirm");
  assert(!pending&&!dialog&&!strcmp(notice,L("The profile list changed. Check it again.","配置文件列表已变化，请重新确认。")));
  capture(b,dir,"esim-stale");stub_job="OK 1\n";notice[0]=0;
  /* While the agent's job runs, nothing else starts. */
  snprintf(data.esim.state,sizeof(data.esim.state),"running");snprintf(data.esim.kind,sizeof(data.esim.kind),"enable");
  snprintf(data.esim.step,sizeof(data.esim.step),"sim_power_cycle");
  capture_both(b,dir,"esim-switching");test_request[0]=0;
  press_id(b,"escan");press_id(b,"e1");assert(!test_request[0]&&!dialog);
  /* The firmware kept the previous profile: a restart is offered, and asks first. */
  snprintf(data.esim.state,sizeof(data.esim.state),"done");data.esim.reboot=1;data.esim.firmware_ready=0;
  capture_both(b,dir,"esim-reboot");
  press_id(b,"ereboot");assert(dialog==5&&confirm_what==CF_REBOOT);capture(b,dir,"esim-reboot-confirm");
  press_id(b,"confirm");expect_request("esim reboot\n");reset_job();
  data.esim.reboot=0;data.esim.firmware_ready=1;render(b);assert(strstr(html,"id=\"ereboot\" href=\"act:none\""));
  capture(b,dir,"esim-switched");
  press_id(b,"escan");expect_request("esim scan\n");reset_job();
  /* A failed job shows the agent's message. */
  snprintf(data.esim.state,sizeof(data.esim.state),"failed");
  snprintf(data.esim.error,sizeof(data.esim.error),"lpac failed at es10c_enable_profile: the card is busy (catBusy)");
  capture(b,dir,"esim-failed");assert(strstr(html,"卡片忙"));
  /* An ordinary SIM, no card read yet (read on entry), and no lpac. */
  data.esim.n=0;data.esim.total=0;snprintf(data.esim.card,sizeof(data.esim.card),"sim");data.esim.state[0]=0;data.esim.error[0]=0;
  capture_both(b,dir,"esim-sim");
  press_id(b,"back");assert(page==MENU);
  data.esim.card[0]=0;test_request[0]=0;press_id(b,"esim");expect_request("esim scan\n");assert(pending_kind==JOB_ESIM);
  reset_job();data.esim.installed=0;capture(b,dir,"esim-missing");test_request[0]=0;press_id(b,"escan");assert(!test_request[0]);
  press_id(b,"back");assert(page==MENU);
  /* Hidden from the menu: not reachable. */
  settings_menu=80;assert(!reachable(ESIM));page=MENU;render(b);assert(!strstr(html,"act:esim"));
  puts("PASS eSIM page (zh/en): W settings record, profile records, switch and restart confirmations, paging, stale generations, running jobs and card states");
}
/* The Wi-Fi relay page: Y settings, the R record, Overview's row, switching and states. */
static void uplink_tests(struct drm_buf *b,const char *dir) {
  char text[4096];
  struct snapshot s,prefs;
  unsigned long long now=(unsigned long long)time(NULL);
  reset_job();
  snprintf(text,sizeof(text),
    "S 7 1 0 0 0 0 0 0 1 %llu\nO 83 320 18372 34.5 12.4\n"
    "B\toff\t0\t7\t1\t-\t0\t12\t1\t-\t0\n"
    "R\t6\t1\tn\t-\t\xF0\x9F\x8F\xA8 Hotel 酒店 5G\t-58\t5\t192.168.50.23\t734003200\t52428800\t%llu\t2\n"
    "C 50 90 200 2 27 zh\nV 50 90 200 2 112 zh\nW 50 90 200 2 208 zh 1\nY 50 90 200 2 464 zh 1\nJ 0 5 5 0 5\n.\n",now,now-3725);
  assert(parse_snapshot(text,&s)&&s.uplink.has&&s.uplink.enabled&&s.uplink.state=='n'&&s.uplink.rev==6);
  assert(!strcmp(s.uplink.ssid,"Hotel 酒店 5G")&&s.uplink.has_signal&&s.uplink.signal==-58&&!strcmp(s.uplink.band,"5"));
  assert(!strcmp(s.uplink.ip,"192.168.50.23")&&s.uplink.saved==2&&!strcmp(s.uplink.reason,""));
  data=s;assert(update_settings(&data)&&settings_menu==464&&visible_page(UPLINK)&&visible_page(ESIM)&&!visible_page(PROXY));
  /* Y wins over W; without Y (or with a bad one) W applies, which has no relay. */
  char no_y[]="W 51 90 200 2 240 zh 1\n.\n",bad_y[]="W 52 90 200 2 240 zh 1\nY 52 90 200 2 1488 zh 1\n.\n";
  char short_y[]="W 53 90 200 2 240 zh 1\nY 53 90 200 2 464 zh\n.\n",home_y[]="W 54 90 200 2 240 en 1\nY 54 90 200 8 464 en 0\n.\n";
  assert(parse_snapshot(no_y,&prefs)&&update_settings(&prefs)&&settings_menu==240&&!visible_page(UPLINK));
  assert(parse_snapshot(bad_y,&prefs)&&update_settings(&prefs)&&settings_menu==240);
  assert(parse_snapshot(short_y,&prefs)&&update_settings(&prefs)&&settings_menu==240);
  assert(parse_snapshot(home_y,&prefs)&&update_settings(&prefs)&&settings_home==UPLINK&&!settings_zh&&!settings_sleep);
  /* A malformed R record is ignored. */
  char bad_r[]="V 55 90 200 2 112 zh\nR\t6\t1\tz\t-\tX\t-\t-\t-\t0\t0\t0\t1\nR\t6\t1\tn\n.\n";
  assert(parse_snapshot(bad_r,&prefs)&&!prefs.uplink.has);
  settings_zh=1;settings_menu=464;
  page=MENU;capture_both(b,dir,"menu-uplink");assert(strstr(html,"act:uplink")&&strstr(html,"Hotel"));
  press_id(b,"uplink");assert(page==UPLINK);capture_both(b,dir,"uplink");
  assert(strstr(html,"192.168.50.23")&&strstr(html,"Traffic: Wi-Fi")&&strstr(html,"-58 dBm"));
  /* Turning off asks first and names the revision shown. */
  press_id(b,"utoggle");assert(dialog==5&&confirm_what==CF_UPLINK&&!confirm_target&&confirm_rev==6);
  capture_both(b,dir,"uplink-confirm-off");
  press_id(b,"confirm");expect_request("uplink 6 off\n");assert(pending_kind==JOB_UPLINK&&!dialog&&job_wait_ms(JOB_UPLINK)==60000);
  capture(b,dir,"uplink-applying");assert(strstr(html,"正在应用"));
  /* A newer revision saved elsewhere answers a conflict. */
  data.job=data.done=data.fresh=pending;data.has_fresh=1;data.result=1;data.failed_job=pending;
  snprintf(data.failure,sizeof(data.failure),"Configuration changed; reload and try again");
  settle_jobs();assert(dialog==3&&!pending&&strstr(notice,"设置已被更改"));capture(b,dir,"uplink-conflict");
  press_id(b,"refresh");assert(!dialog&&page==UPLINK);reset_job();data.result=0;
  /* States: wrong password while connecting, no internet, unavailable, off. */
  data.uplink.state='c';snprintf(data.uplink.reason,sizeof(data.uplink.reason),"password");data.uplink.ip[0]=0;data.uplink.has_signal=0;
  data.uplink.ssid[0]=0;capture_both(b,dir,"uplink-password");assert(!strstr(html,"Traffic: Wi-Fi")&&strstr(html,"Wrong Wi-Fi password"));
  data.uplink.state='i';snprintf(data.uplink.reason,sizeof(data.uplink.reason),"internet");
  snprintf(data.uplink.ssid,sizeof(data.uplink.ssid),"Cafe");snprintf(data.uplink.ip,sizeof(data.uplink.ip),"10.0.0.8");
  capture_both(b,dir,"uplink-nointernet");assert(strstr(html,"no internet")&&strstr(html,"id=\"ulogin\" href=\"act:ulogin\""));
  /* A web login: login mode starts at once (no confirmation), then traffic is on Wi-Fi. */
  data.uplink.state='p';snprintf(data.uplink.reason,sizeof(data.uplink.reason),"portal");
  capture_both(b,dir,"uplink-portal");assert(strstr(html,"web login")&&strstr(html,"Start login"));
  press_id(b,"ulogin");expect_request("uplink login\n");assert(pending_kind==JOB_UPLINK&&!dialog);reset_job();
  data.uplink.state='g';data.uplink.reason[0]=0;
  capture_both(b,dir,"uplink-login");assert(strstr(html,"Traffic: Wi-Fi")&&strstr(html,"id=\"ulogin\" href=\"act:none\""));
  data.uplink.state='i';snprintf(data.uplink.reason,sizeof(data.uplink.reason),"internet");
  data.uplink.state='u';snprintf(data.uplink.reason,sizeof(data.uplink.reason),"supplicant");data.uplink.ssid[0]=0;data.uplink.ip[0]=0;
  capture(b,dir,"uplink-unavailable");assert(strstr(html,"wpa_supplicant"));
  data.uplink.state='o';data.uplink.enabled=0;data.uplink.reason[0]=0;data.uplink.rx=data.uplink.tx=0;
  capture_both(b,dir,"uplink-off");
  press_id(b,"utoggle");assert(dialog==5&&confirm_target==1);capture_both(b,dir,"uplink-confirm-on");
  press_id(b,"confirm");expect_request("uplink 6 on\n");reset_job();
  /* Nothing saved: turning on is refused here, networks are added on the webpage. */
  data.uplink.saved=0;render(b);assert(strstr(html,"id=\"utoggle\" href=\"act:none\""));capture(b,dir,"uplink-empty");
  test_request[0]=0;press_id(b,"utoggle");assert(!test_request[0]&&!dialog);
  data.uplink.saved=2;
  /* Stale status disables the switch. */
  data.stamp=now-20;render(b);assert(strstr(html,"id=\"utoggle\" href=\"act:none\""));data.stamp=now;
  press_id(b,"back");assert(page==MENU);
  /* Overview shows the relay without the menu item and goes back to Overview. */
  settings_menu=240;data.uplink.state='n';data.uplink.enabled=1;snprintf(data.uplink.ssid,sizeof(data.uplink.ssid),"Hotel 酒店 5G");
  assert(!visible_page(UPLINK)&&reachable(UPLINK));
  page=MENU;render(b);assert(!strstr(html,"act:uplink"));
  settings_menu=240;page=OVERVIEW;capture_both(b,dir,"overview-uplink");
  press_id(b,"uplink");assert(page==UPLINK&&uplink_from==OVERVIEW);press_id(b,"back");assert(page==OVERVIEW);
  /* An agent without the relay: no row, and the page is not reachable. */
  data.uplink.has=0;assert(!reachable(UPLINK));render(b);assert(!strstr(html,"act:uplink"));
  page=UPLINK;track_status(0);assert(page==MENU);
  puts("PASS Wi-Fi relay page (zh/en): Y settings record, R record, menu and Overview rows, on/off confirmations, conflicts and states");
}
static void toolbox_tests(struct drm_buf *b, const char *dir) {
  char text[4096];
  struct snapshot s;
  unsigned long long now=(unsigned long long)time(NULL);
  snprintf(text,sizeof(text),
    "S 7 1 0 0 0 0 0 0 1 %llu\nO 83 320 18372 34.5 12.4\n"
    "H\t-65\t-88\t-10\t19\t5G SA\tN78\tWL_AND_5G\tB1,B3\tN78,N41\t502\t256000\t42000\t12\t35\t34200\t2\t1\t1\t5\tU60Pro_5G\trndis\t0\t64\t1\t1\t80\t80\t70\t4120\t450\tGood\n"
    "Z 50 90 200 2 16096 zh 1\nJ 0 6 6 0 6\n.\n",now);
  assert(parse_snapshot(text,&s)&&s.toolbox.has&&s.toolbox.rsrp==-88&&s.toolbox.ttl_val==64&&s.toolbox.wifi_clients==5);
  data=s;assert(update_settings(&data)&&settings_menu==16096);
  page=MENU;capture_both(b,dir,"menu-toolbox-3x3");
  assert(strstr(html,"class=\"box tile\"")&&strstr(html,"act:wifi")&&strstr(html,"act:signal")&&strstr(html,"act:usb")&&strstr(html,"act:power")&&strstr(html,"act:booster"));
  press_id(b,"wifi");assert(page==WIFI_PAGE);capture_both(b,dir,"toolbox-wifi");
  press_id(b,"w2off");expect_request("tool wifi_toggle 2g 0\n");assert(pending_kind==JOB_TOOL);reset_job();
  press_id(b,"back");assert(page==MENU);
  press_id(b,"signal");assert(page==SIGNAL_PAGE);capture_both(b,dir,"toolbox-signal");
  assert(strstr(html,"5G Auto"));
  press_id(b,"net1");expect_request("tool net_mode Only_5G\n");reset_job();
  press_id(b,"back");assert(page==MENU);
  press_id(b,"usb");assert(page==USB_PAGE);capture_both(b,dir,"toolbox-usb");
  press_id(b,"ttl65");expect_request("tool ttl 65\n");reset_job();
  press_id(b,"usb1");expect_request("tool usb_mode ncm\n");reset_job();
  assert(!strstr(html,"usbadb"));
  press_id(b,"back");assert(page==MENU);
  press_id(b,"power");assert(page==POWER_PAGE);capture_both(b,dir,"toolbox-power");
  press_id(b,"lim90");expect_request("tool charge_limit 90\n");reset_job();
  press_id(b,"back");assert(page==MENU);
  press_id(b,"booster");assert(page==BOOSTER_PAGE);capture_both(b,dir,"toolbox-booster");
  press_id(b,"killbloat");expect_request("tool kill_bloat\n");reset_job();
  press_id(b,"dropcache");expect_request("tool drop_caches\n");reset_job();
  press_id(b,"back");assert(page==MENU);
  puts("PASS Pocket OS 3x3 Toolbox grid and Wi-Fi, 5G Signal, USB & TTL, Battery & Charge, and Booster pages (zh/en)");
}
int main(int argc, char **argv) {
  power_self_test();
  if(argc<2)return 2;
  struct drm_buf b={.pitch=W*2,.map=calloc(W*H,2)};
  assert(b.map);
  struct drm_buf padded={.pitch=(W+3)*2,.map=calloc((W+3)*H,2)};
  assert(padded.map);padded.map[0]=1;padded.map[W-1]=2;
  padded.map[(W+3)*(H-1)]=3;padded.map[(W+3)*(H-1)+W-1]=4;padded.map[W]=0x1234;
  rotate_frame(&padded);
  assert(padded.map[0]==4&&padded.map[W-1]==3&&padded.map[W]==0x1234);
  rotate_frame(&padded);assert(padded.map[0]==1);free(padded.map);
  char example[4096];
  snprintf(example,sizeof(example),
    "S 7 1 1 1 0 %llu 134217728 25165824 1 %llu\n"
    "D\t02:11:22:33:44:55\t1\t1\t192.0.2.2\tWork laptop\n"
    "D\t02:11:22:33:44:56\t0\t1\t192.0.2.3\tiPhone\n"
    "D\t02:11:22:33:44:57\t1\t0\t-\t-\n"
    "D\t02:11:22:33:44:58\t0\t0\t-\tWork laptop\n"
    "D\t02:11:22:33:44:59\t0\t0\t-\t<a href='act:stock'> A very long device name\n"
    "P\t1\t1\tHome VPN\nP\t2\t0\tTravel VPN\nP\t3\t0\tWork VPN\n"
    "P\t4\t0\tBackup VPN\nP\t5\t0\tA long profile name <a href='act:stock'>\n"
    "O 83 320 18372 34.5 12.4\nJ 0 0 0 0\n.\n",
    (unsigned long long)time(NULL)-15,(unsigned long long)time(NULL));
  assert(parse_snapshot(example,&data));assert(data.n==5&&data.np==5&&data.battery==83);
  capture(&b,argv[1],"menu");press_id(&b,"overview");assert(page==OVERVIEW);
  capture(&b,argv[1],"overview");press_id(&b,"back");assert(page==MENU);
  press_id(&b,"wireguard");assert(page==WG);capture(&b,argv[1],"home");
  rotate_frame(&b);char path[512];snprintf(path,sizeof(path),"%s/home-panel.ppm",argv[1]);dump(&b,path);
  press_id(&b,"devices");assert(page==DEVICES);press_id(&b,"d1");assert(dirty&&draft.rows[1].selected);
  capture(&b,argv[1],"devices");press_id(&b,"next");assert(offset==4);
  capture(&b,argv[1],"devices-page2");assert(strstr(html,"&lt;a href="));
  press_id(&b,"prev");assert(offset==0);press_id(&b,"back");assert(dialog==2);
  capture(&b,argv[1],"unsaved");press_id(&b,"saveback");assert(strstr(test_request,"devices 7 ")&&pending==1);
  pending=0;dirty=0;dialog=0;page=WG;
  press_id(&b,"toggle");assert(dialog==1);capture(&b,argv[1],"confirm");press_id(&b,"confirm");
  assert(!strcmp(test_request,"toggle 7 0\n"));pending=0;dialog=0;
  press_id(&b,"profiles");assert(page==PROFILES);capture(&b,argv[1],"profiles");
  press_id(&b,"p4");assert(chosen_profile==5&&chosen_revision==7&&dialog==4);
  capture(&b,argv[1],"profile-confirm");data.rev=8;
  press_id(&b,"confirm");assert(!strcmp(test_request,"profile 7 5\n"));
  pending=0;dialog=3;snprintf(notice,sizeof(notice),"%s",L("Settings changed. Refresh.","设置已被更改，请刷新。"));capture(&b,argv[1],"conflict");
  press_id(&b,"refresh");assert(page==WG&&!dirty&&!dialog);
  data.has=0;data.enabled=0;data.connected=0;capture(&b,argv[1],"unconfigured");
  test_request[0]=0;press_id(&b,"toggle");assert(!test_request[0]);
  data.has=1;capture(&b,argv[1],"off");
  data.enabled=1;data.handshake=0;capture(&b,argv[1],"waiting");
  data.error=1;capture(&b,argv[1],"error");data.error=0;
  data.stamp=(unsigned long long)time(NULL)-20;capture(&b,argv[1],"stale");
  press_id(&b,"toggle");assert(!test_request[0]);
  char settings_line[]="C 3 60 200 4 16\n.\n";
  struct snapshot prefs; assert(parse_snapshot(settings_line,&prefs));
  assert(update_settings(&prefs)==2 && settings_idle==60 && settings_brightness==200 && settings_home==OVERVIEW);
  assert(!update_settings(&prefs));
  page=MENU;capture(&b,argv[1],"menu-custom");
  assert(strstr(html,"act:overview") && !strstr(html,"act:wireguard"));
  press_id(&b,"overview");assert(page==OVERVIEW);press_id(&b,"wireguard");assert(page==OVERVIEW);
  data.battery=12;capture(&b,argv[1],"battery-low");assert(strstr(html,"12%") && strstr(html,"#ff8a8a"));
  data.battery=-1;capture(&b,argv[1],"battery-unknown");assert(strstr(html,"--%"));
  char bad_settings[]="C 4 0 0 4 99\n.\n";assert(!parse_snapshot(bad_settings,&prefs));
  /* The language field of the C record switches every page; Chinese is the default. */
  assert(strstr(html,"lang=\"zh\"")&&strstr(html,"电池"));
  char bad_language[]="C 5 60 200 2 27 fr\n.\n";assert(!parse_snapshot(bad_language,&prefs));
  char english[]="C 5 60 200 2 27 en\n.\n";
  assert(parse_snapshot(english,&prefs)&&update_settings(&prefs)==1&&!settings_zh&&settings_menu==27);
  data.battery=83;data.stamp=(unsigned long long)time(NULL);data.handshake=data.stamp-15;
  page=MENU;capture(&b,argv[1],"en-menu");assert(strstr(html,"lang=\"en\"")&&strstr(html,"Stock UI")&&!strstr(html,"原厂"));
  page=OVERVIEW;capture(&b,argv[1],"en-overview");page=WG;capture(&b,argv[1],"en-home");
  page=PROFILES;capture(&b,argv[1],"en-profiles");enter_devices();capture(&b,argv[1],"en-devices");
  dialog=1;capture(&b,argv[1],"en-confirm");dialog=0;
  char legacy[]="C 6 60 200 2 27\n.\n"; /* An agent without the field keeps Chinese. */
  assert(parse_snapshot(legacy,&prefs)&&update_settings(&prefs)==1&&settings_zh);
  page=MENU;capture(&b,argv[1],"menu-zh");assert(strstr(html,"原厂界面"));
  page=MENU;press_id(&b,"stock");assert(quitting);quitting=0;
  puts("PASS DevUI HTML hit testing, menu navigation, profile version capture, device pagination/drafts, escaping, confirmations, disabled states and rotation");
  proxy_tests(&b,argv[1]);
  esim_tests(&b,argv[1]);
  uplink_tests(&b,argv[1]);
  toolbox_tests(&b,argv[1]);
  free(b.map);
  return 0;
}
#endif
