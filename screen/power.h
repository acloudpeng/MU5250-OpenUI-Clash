/* SPDX-License-Identifier: GPL-3.0-or-later */
/* Event timestamps, not poll time: queued long presses must remain long. */
struct power_button { long long pressed, released; int down, dropped, held; };
/* A press held this long opens the power menu. */
#define POWER_HOLD_MS 1500
enum { POWER_NONE, POWER_SHORT, POWER_LONG };
/* POWER_SHORT on the release of a short press; POWER_LONG on the release of a long one
 * that power_hold has not already reported (both events read late, at once). */
static int power_event(struct power_button *k, int type, int code, int value,
                       long long stamp) {
  if (type == 0 && code == 3) { /* SYN_DROPPED */
    k->down = 0; k->dropped = 1; return 0;
  }
  if (k->dropped) {
    if (type == 0 && code == 0) k->dropped = 0;
    return 0;
  }
  if (type != 1 || code != 116) return 0; /* EV_KEY, KEY_POWER */
  if (value == 1 && !k->down) { k->down = 1; k->held = 0; k->pressed = stamp; }
  if (value != 0 || !k->down) return 0;
  k->down = 0;
  long long duration = stamp - k->pressed;
  if (duration >= POWER_HOLD_MS) {
    k->released = stamp;
    return k->held ? POWER_NONE : POWER_LONG;
  }
  int short_press = duration >= 30 && duration < 800 &&
                    (!k->released || k->pressed - k->released >= 80);
  k->released = stamp;
  return short_press ? POWER_SHORT : POWER_NONE;
}
/* Once per press, while it is still held: the key has been down POWER_HOLD_MS by `now`
 * (the same clock as the event stamps). */
static int power_hold(struct power_button *k, long long now) {
  if (!k->down || k->held || now - k->pressed < POWER_HOLD_MS) return 0;
  k->held = 1;
  return 1;
}
static void power_self_test(void) {
  struct power_button k = {0};
  assert(!power_event(&k, 1, 116, 0, 100)); /* Opened while held */
  assert(!power_event(&k, 1, 116, 1, 1000));
  assert(!power_event(&k, 1, 116, 2, 1100)); /* Repeat */
  assert(power_event(&k, 1, 116, 0, 1200));
  assert(!power_event(&k, 1, 116, 0, 1201));
  assert(!power_event(&k, 1, 116, 1, 1220));
  assert(!power_event(&k, 1, 116, 0, 1260)); /* Bounce */
  assert(!power_event(&k, 1, 116, 1, 2000));
  assert(!power_event(&k, 1, 116, 0, 2800)); /* Long press */
  assert(!power_event(&k, 1, 116, 1, 3000));
  assert(!power_event(&k, 0, 3, 0, 3050));
  assert(!power_event(&k, 1, 116, 0, 3100));
  assert(!power_event(&k, 0, 0, 0, 3200));
  assert(!power_event(&k, 1, 116, 0, 3300));
  assert(!power_event(&k, 1, 116, 1, 4000));
  assert(power_event(&k, 1, 116, 0, 4100) == POWER_SHORT);
  /* Held: reported once while down, and its release is neither short nor long. */
  assert(!power_event(&k, 1, 116, 1, 5000));
  assert(!power_hold(&k, 6400) && power_hold(&k, 6500) && !power_hold(&k, 6600));
  assert(!power_event(&k, 1, 116, 2, 6600) && !power_hold(&k, 7000));
  assert(power_event(&k, 1, 116, 0, 7200) == POWER_NONE && !power_hold(&k, 9000));
  /* Press and release read together, late: still a long press. */
  assert(!power_event(&k, 1, 116, 1, 10000));
  assert(power_event(&k, 1, 116, 0, 11600) == POWER_LONG);
  /* Dropped events forget a held key. */
  assert(!power_event(&k, 1, 116, 1, 12000));
  assert(!power_event(&k, 0, 3, 0, 12100) && !power_hold(&k, 14000));
  assert(!power_event(&k, 0, 0, 0, 12200));
}
