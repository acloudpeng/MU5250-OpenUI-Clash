# B31 Root / ADB unlock — current status & how-to

_Checked 2026-10-09 against this repo's docs and upstream `jesther-ai/open-u60-pro`
issue #8. Target device: ZTE U60 Pro (MU5250), firmware `CN_ZTE_MU5250V1.0.0B31`._

## TL;DR

- The old one-click web ADB toggle (`zwrt_bsp.usb.set {"mode":"debug"}`) was
  **removed in CN B28 and is gone on B31**. No web trick re-enables it.
- Root is **still obtainable** via the **config backup/restore path** — this repo's
  `scripts/zunlock.py` (and `setup.sh`, which runs it automatically for a locked
  device). Upstream maintainer `dklasens` confirmed this works for B28+ and
  documents it in `COMMUNITY-UNLOCK.md`.
- It needs the **backup-key suffix**. That value is kept out of git on purpose; it is
  in the local, gitignored **`./.env.local`** (`export ZTE_BACKUP_SUFFIX=…`).
- **B31 is unverified upstream** (only HK B04 was). The method does not depend on the
  removed toggle, so it should work — but confirm with the **no-risk dry-run** first.

## Why the old method is gone

Per upstream issue #8 ("cannot work anymore on B28", open): B28 removed the
`zwrt_bsp.usb.set {"mode":"debug"}` RPC (it now returns `{3}` instead of `{1}`).
B31 inherits this. Attempts in the thread to find an alternative engineering/USB RPC
(`web_debug_mode_set`, `web_developer_option_login`, …) did not yield a working web
path.

## The method that still works

Config backup/restore (this repo's `scripts/zunlock.py`):

- Documented upstream: `dklasens/MU5250-OpenUI/blob/main/COMMUNITY-UNLOCK.md`
  (dklasens, 2026-07-20 in issue #8).
- Implemented here: `scripts/zunlock.py` + `setup.sh` (auto-runs it when it detects a
  locked device). See `docs/DEPLOYMENT.md` §1 "Unlock (locked firmware: HK B04+, CN B28+)".
- It does **not** rely on the removed toggle, so it is firmware-version-agnostic in
  principle.

## The backup-key suffix

- Full backup password = `<device IMEI>` + a shared SDX75-family **suffix**.
  `zunlock.py` reads the IMEI from the device; you supply only the suffix
  (`--suffix` or the `ZTE_BACKUP_SUFFIX` env var).
- Source: upstream issue #8, `@iangcarroll` (2026-08-23), stated "on the MU5252 at
  least" → **confirmed for MU5252; MU5250 / B31 not yet confirmed**. A wrong suffix
  simply fails the dry-run decrypt — harmless.
- **Location in this checkout:** `./.env.local` (gitignored).
- **Never commit it.** This repo is verified clean of key material across all history;
  the researchers (`amenekowo/mu5250_tweaking`) asked that it not be republished, and
  publishing it would get it patched in the next firmware. `.env.local`,
  `adb-lock-investigation.md`, `back_parameter*` are gitignored for exactly this reason.

## B31 caveat

`docs/PLAN.md` risk #5: "B31 解锁流程未经上游验证（上游只验证 HK B04）", and P0 was
blocked on the suffix. The suffix is now available (above), so the only remaining
unknown is whether it + the backup/restore handshake succeed on B31 — which the
dry-run answers without changing anything.

## How to check — zero risk to the device

Run **from the computer physically connected to the U60** (its Wi-Fi or USB, reaching
`192.168.0.1`). **Not** from the remote IDC VM — that VM cannot see the device.

```sh
set -a; . ./.env.local; set +a          # load ZTE_BACKUP_SUFFIX (gitignored)
python3 scripts/zunlock.py --dry-run     # login -> pull backup -> decrypt -> repackage; NO upload, NO device change
```

- Completes cleanly → the suffix is right and the backup/restore handshake works on
  B31 → the real unlock will work.
- Fails at decrypt ("wrong suffix…") → the suffix does not apply to this unit/firmware;
  stop and re-source it.

Alternatively, the GUI installer's **Check device** step reports model / firmware /
locked-state without changing anything.

## Full unlock + install (only after a clean dry-run, accepting a ~90 s reboot)

```sh
# from the machine with the device:
bash setup.sh            # detects a locked device -> runs the unlock -> agent + SSH + FOTA off
bash deploy-dashboard.sh # web dashboard
# Clash / WireGuard components: build + scripts/deploy-components.py — see docs/CLASH.md
```

Settings are preserved (the patched backup is built from one taken seconds earlier).
The device reboots and is offline for ~90 s.

## Safety red lines (`docs/SAFETY.md` — this model was bricked once, 2026-07)

- Always `--dry-run` first.
- **No partition writes** (`dd` / `mtd` / `fw_setenv` / fuse / `abctl --set_active`).
- Do **not** disable `zte_topsw_daemon.conf` daemons via `init.d` (boot sync-barrier brick).
- Keep **FOTA auto-update off** (a surprise firmware update wipes the rc.local lines and
  can change / patch the unlock).
- `scripts/research/` stays quarantined — it is **not** part of the unlock path.

## Sources

- Upstream issue: `jesther-ai/open-u60-pro#8` — B28 breakage, dklasens' COMMUNITY-UNLOCK.md
  pointer, iangcarroll's suffix.
- Upstream how-to: `dklasens/MU5250-OpenUI/blob/main/COMMUNITY-UNLOCK.md`.
- This repo: `docs/DEPLOYMENT.md` §1, `docs/SAFETY.md`, `docs/PLAN.md` (P0, risk #5),
  `scripts/zunlock.py`, `setup.sh`.
