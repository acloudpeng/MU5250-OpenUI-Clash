# AI Operations Runbook — MU5250 (ZTE U60 Pro) unlock, deploy, firmware update

> **中文摘要**：本文是给 AI 代理执行的结构化手册，用状态机 + 精确命令 + 验收门槛的
> 形式写成。覆盖：① 识别设备当前状态 ② 解锁（取得 root/ADB）③ 部署 OpenUI
> ④ 验证 ⑤ 固件更新与事后重建。**所有密钥都是运行时输入，仓库内不含任何密钥。**
> 关键红线在第 8 节，执行前必读。

Machine-oriented. Optimised for an AI agent that must execute without guessing.
Every command is copy-pasteable. Every step states its expected output and its
abort condition.

Target: ZTE U60 Pro **MU5250** (SDX75 / SDXPINN), firmware family `CN_ZTE_MU5250V1.0.0B*`.
Validated on `BD_CNMU5250V1.0.0B31` (2026-10).

---

## 0. Contract for the executing agent

| Rule | Detail |
|---|---|
| **Never guess secrets** | Router admin password and `ZTE_BACKUP_SUFFIX` are runtime inputs. If absent, **stop and ask**. |
| **Never invent a suffix** | A wrong suffix fails harmlessly at decrypt; a *guessed* one wastes a device reboot cycle. |
| **Always dry-run first** | `zunlock.py --dry-run` is read-only. It must pass before any write. |
| **Never construct device shell commands inline through a host shell** | See §7.1 — this is the single most dangerous host mistake. |
| **One writer at a time** | Do not run two deployment tools concurrently. The transaction lock will refuse, or worse, interleave. |
| **Report state, not narrative** | After each phase, print the state token from §1 plus the evidence lines. |

### Evidence discipline

For every phase, capture and report:

```
PHASE: <name>
STATE_BEFORE: <token>
COMMANDS: <exact argv used>
STATE_AFTER: <token>
EVIDENCE: <raw command output, unedited>
GATE: PASS | FAIL
```

---

## 1. State machine

Six states. Detect with the probes below, in order, and stop at the first match.

```
S0  HOST_UNPREPARED        no adb, no python crypto, no repo checkout
S1  LOCKED                 device reachable, root shell NOT available
S2  UNLOCKED               root shell available, OpenUI NOT deployed
S3  DEPLOYED               OpenUI deployed, reboot NOT yet verified
S4  PERSISTENT             OpenUI deployed AND verified across a reboot
S5  POST_FOTA_DEGRADED     firmware updated; /data intact, /etc hooks gone
```

### Detection probes

**P1 — device reachable?**
```sh
# from the host, on the device's LAN or USB tether
curl -s -o /dev/null -w '%{http_code}\n' --max-time 8 http://192.168.0.1/
# expect: 200   (stock web UI)
```

**P2 — root shell available?**
```sh
adb devices                     # a serial in 'device' state, OR
ssh -p 2222 -o BatchMode=yes -o ConnectTimeout=8 root@192.168.0.1 'id -u'
# expect: 0
```

**P3 — which USB composition?**
```sh
# host side
#   VID_19D2 PID_1404  -> debug composition (ADB present)
#   VID_19D2 PID_1403  -> stock composition (tethering only)
lsusb | grep 19d2            # linux
# windows: Get-CimInstance Win32_PnPEntity | ? PNPDeviceID -like '*VID_19D2*'
```
```sh
# device side (needs root)
cat /sys/class/android_usb/android0/usb_op
#   1 or 2 -> flash-protect OFF (debug/unlocked this boot)
#   0      -> stock
```

**P4 — deployed?**
```sh
# over ADB or SSH
test -x /data/zte-agent && echo AGENT
test -x /data/bin/mihomo && echo MIHOMO
grep -c 'start_zte_agent.sh' /etc/rc.local    # expect 1 when deployed
```

**P5 — persistence verified?**
```sh
cat /proc/uptime | cut -d. -f1     # if the device booted AFTER deployment and
                                   # S3 artifacts are present -> S4
```

### Transition map

```
S0 --prereqs--> S1
S1 --zunlock--> S2
S2 --deploy--> S3
S3 --reboot+verify--> S4
any --firmware update--> S5
S5 --recover--> S4
```

---

## 2. Prerequisites (S0 → ready)

### 2.1 Host tools

| Tool | Needed for | Check |
|---|---|---|
| `adb` | unlock, deploy transport | `adb version` |
| Python 3.8+ | `zunlock.py`, deploy scripts | `python3 -V` |
| `cryptography` **or** `openssl` CLI | backup decrypt/encrypt | see below |
| `ssh` + `ssh-keygen` | key-only SSH transport | `ssh -V` |
| `curl` | package fetch | `curl --version` |

`zunlock.py` prefers the `cryptography` package and falls back to the `openssl`
binary. Either satisfies it:

```sh
python3 -m pip install cryptography          # preferred, no openssl needed
# OR
openssl version                              # must support des-ede3-cbc + sha256
```

> **Verify before relying on it.** `des-ede3-cbc` is Triple-DES. In
> `cryptography >= 43` it moved to `cryptography.hazmat.decrepit...`; the script
> already handles both locations. Self-test:
> ```sh
> python3 - <<'PY'
> import sys; sys.path.insert(0, 'scripts')
> import zunlock
> d = b'x' * 100; pw = 'test-password'
> e = zunlock._py_crypt(d, pw, encrypt=True)
> assert zunlock._py_crypt(e, pw, encrypt=False) == d
> try: zunlock._py_crypt(e, pw + 'X', encrypt=False); raise SystemExit('FAIL: wrong pw accepted')
> except ValueError: print('crypto OK')
> PY
> ```

### 2.2 Secrets (all runtime, never committed)

| Name | Where it comes from | Notes |
|---|---|---|
| Router admin password | device owner; set at first boot | used for web UI login |
| `ZTE_BACKUP_SUFFIX` | device-family constant, **not in this repo** | see `docs/DEPLOYMENT.md` for the acquisition path |
| Agent dashboard password | chosen at deploy time | becomes the OpenUI login |

Provide via env, never on the command line (shell history):

```sh
export ROUTER_PW='<router admin password>'
export ZTE_BACKUP_SUFFIX='<suffix>'          # or a gitignored .env.local
```

The repo's `scripts/check-device-secrets.py` runs in CI and **rejects** tracked
files containing a literal suffix, a device IMEI, private keys or tokens. Do not
paste real values into any file under version control.

### 2.3 Confirm the target model

```sh
curl -s http://192.168.0.1/ubus/ -H 'Content-Type: application/json' \
  -d '[{"jsonrpc":"2.0","id":1,"method":"call","params":["00000000000000000000000000000000","zwrt_web","device_info",{}]}]'
# expect: JSON containing "wa_inner_version":"BD_CNMU5250V1.0.0B.."
```

Abort if the model is not MU5250/MU5252 and the firmware is not `*MU5250V*`.

---

## 3. UNLOCK — S1 → S2

### 3.1 Why the old web toggle does not work

Firmware **B28 and later removed** the `zwrt_bsp.usb.set {"mode":"debug"}` RPC
(returns `{3}` instead of `{1}`). No web/ubus trick re-enables ADB on B28/B31.

The working path is **configuration backup → patch → restore**, because the
restore process runs as root and extracts whatever it is given.

### 3.2 Mechanism (for verification, not memorisation)

```
1. log in to the stock web UI          -> ubus session
2. ubus call zwrt_mc.device.manager device_backup_proc {"procType":"web"}
3. GET /backup/back_parameter           -> openssl-encrypted tar
     cipher : des-ede3-cbc, digest sha256, "Salted__" container
     passwd : <device IMEI><suffix>
4. decrypt -> outer tgz -> tmp/back_parameter_r1.tgz -> etc/rc.local
5. insert ONE line after the shebang:
       echo 1 > /sys/class/android_usb/android0/usb_op
   The sysfs path is DISCOVERED from the device's own rc.local
   (regex over existing usb_op references), never hard-coded.
6. repack inner tgz, refresh tmp/back_parameter_r.md5, repack outer, re-encrypt
7. POST /cgi-bin/cgi-upload   (server returns sha256sum; MUST match locally)
8. ubus call zwrt_mc.device.manager device_restore_proc {"procType":"web"}
9. device restores and reboots -> adbd is up
```

### 3.3 Execute

```sh
cd <repo>/scripts

# STEP 1 — read-only rehearsal. Must pass.
ROUTER_PW="$ROUTER_PW" ZTE_BACKUP_SUFFIX="$ZTE_BACKUP_SUFFIX" \
  python3 zunlock.py --dry-run --keep-work /tmp/unlock

# STEP 2 — verify the patched package BEFORE uploading (see 3.4)
# STEP 3 — real unlock
ROUTER_PW="$ROUTER_PW" ZTE_BACKUP_SUFFIX="$ZTE_BACKUP_SUFFIX" \
  python3 zunlock.py --yes --keep-work /tmp/unlock
```

Expected dry-run output:
```
[*] logging in...
[+] device IMEI: <6 digits>*********          <- masked; never log in full
[*] requesting fresh config backup...
[+] backup downloaded: <n> bytes
[+] Original encrypted recovery backup retained: <path>
[*] decrypting (des-ede3-cbc/sha256)...
[+] payload after shebang: echo 1 > /sys/class/android_usb/android0/usb_op
[+] patched package ready: <n> bytes, sha256 <16 hex>...
[*] dry-run: stopping before upload.
```

### 3.4 Mandatory package verification

Never upload a package you have not diffed. Required invariants:

| # | Invariant | Why |
|---|---|---|
| 1 | inner-tgz MD5 sidecar == actual inner tgz MD5 | restore validates this |
| 2 | outer members unchanged in count and content | a truncated member is a corrupt restore |
| 3 | **exactly one** member differs: `etc/rc.local` | anything else is scope creep |
| 4 | `rc.local` diff is **+1 line, -0 lines** | a deletion can break boot |
| 5 | member mode/uid/gid/type/linkname preserved | symlink and perm drift breaks extraction |
| 6 | the stock flash-protect block that **READS** `usb_op` is still present | see §8 red line 3 |
| 7 | injected line sits immediately after the shebang | stock block reads the node right after |

Reference implementation of checks 1–7: see `tests/` patterns and the
`verify_package.py` style used by the maintainers. tar member *order* may change
(rc.local can move to the end); this is harmless when there are no hardlinks or
device nodes, which is the case here.

### 3.5 Gate

```
GATE: root shell reachable
  adb devices                       -> <serial>  device
  adb shell id -u                   -> 0
  adb shell uname -m                -> aarch64
  adb shell cat /sys/class/android_usb/android0/usb_op  -> 1
```

Failure handling:

| Symptom | Meaning | Action |
|---|---|---|
| `decryption failed — wrong suffix` | suffix invalid for this unit | stop; re-source the suffix |
| `router login failed` | wrong admin password / no route | stop; confirm password and reachability |
| upload sha256 mismatch | transport corruption | **aborted before restore**; retry |
| restore triggered but no ADB after 3 min | booted to stock | see §5 recovery; the device is not bricked |

---

## 4. DEPLOY — S2 → S3

### 4.1 Prepare the artifacts

```sh
python3 scripts/prepare-mihomo.py       --output build/mihomo/mihomo     # needs Go or Docker
python3 scripts/prepare-geodata.py      --output build/mihomo/geo
python3 scripts/prepare-wireguard.py    --output build/wireguard/wg
python3 scripts/deploy-components.py --prepare-only --bundle-output build/packages
```

`prepare-mihomo.py` builds a pinned Clash.Meta commit and **verifies the sha256
against a constant in the script**. If the digest differs, do not proceed —
either the toolchain differs or the artifact is wrong.

If the repo publishes CI artifacts, prefer downloading them and verifying against
the pinned constants over building locally.

### 4.2 Dry run

```sh
python3 scripts/deploy-components.py \
  --gateway 192.168.0.1 --auto-adb \
  --identity-file <unlock-work>/identity.json \
  --agent    build/release/zte-agent \
  --dashboard build/release/dashboard-dist.tar.gz \
  --mihomo   build/mihomo/mihomo \
  --mihomo-geodata build/mihomo/geo \
  --wireguard-tools build/wireguard/wg \
  --bundle build/packages --harden --dry-run
```

Expected tail:
```
Verified ZTE MU5250 (BD_CNMU5250V1.0.0B31)
Dry run passed: identity, local artifacts and storage checked; no deployment writes.
```

### 4.3 Real deployment

Remove `--dry-run`. The tool:

1. takes a **transaction snapshot** of every target before writing
2. prints a **recovery command** — capture it verbatim
3. installs under a lock; a failure triggers automatic restore
4. verifies the agent API with the chosen password
5. verifies SSH is **key-only** (password auth must be refused)
6. disables FOTA auto-update

Record from the output:
```
Recovery command for this device: sh /data/local/tmp/open-u60-transaction.sh restore <txn> <fingerprint>
Key-only SSH verified; firmware auto-update is off.
Deployment verified. Recovery snapshot retained on the modem.
```

### 4.4 What lands where

| Path | Content | Partition |
|---|---|---|
| `/data/zte-agent` | agent binary | userdata (survives FOTA) |
| `/data/bin/mihomo` | Clash core | userdata |
| `/data/bin/dropbear`, `dbclient`, `dropbearkey` | SSH | userdata |
| `/data/bin/dashboard-uhttpd` | dashboard server | userdata |
| `/data/bin/wg` | WireGuard tools | userdata |
| `/data/share/mihomo/{geoip.metadb,GeoSite.dat}` | geo data | userdata |
| `/data/open-u60-dashboards/<uuid>` | dashboard assets | userdata |
| `/data/local/tmp/start_{dropbear,dashboard,zte_agent}.sh` | boot scripts | userdata |
| `/data/dropbear/` | **SSH key copies** | userdata |
| `/etc/dropbear/` | live SSH keys | **overlay (FOTA wipes)** |
| `/etc/rc.local` | boot hooks | **overlay (FOTA wipes)** |

> The `/data/dropbear/` copy is deliberate and is the single most valuable
> post-FOTA asset. Verify it exists after deployment.

### 4.5 Gate

```
GATE: services up, agent reachable, SSH key-only
  curl -s http://192.168.0.1:8080/ | grep -q '<div id="root">'      -> found
  POST http://192.168.0.1:9090/api/auth/login {"password":"<agent pw>"} -> {"ok":true,...}
  ssh -p 2222 root@192.168.0.1  (key)                               -> works
  ssh -p 2222 -o PubkeyAuthentication=no root@192.168.0.1           -> REFUSED
```

---

## 5. PERSISTENCE — S3 → S4

**Do not skip this. An unverified installation is not an installation.**

### 5.1 Pre-reboot checks (on the device)

```sh
sh -n /etc/rc.local                       # MUST be silent
wc -c < /etc/rc.local                     # non-zero
for s in start_dropbear.sh start_dashboard.sh start_zte_agent.sh; do
    test -x /data/local/tmp/$s || echo "MISSING $s"
done
sync; sync
```

### 5.2 Reboot and verify

```sh
adb reboot        # or: ssh ... 'reboot'
```

Expect the device back in **60–180 s**. Then:

```sh
sh -c 'netstat -lntp | grep LISTEN' | grep -E '2222|8080|9090|:80 '
ps w | grep -E "zte-agent|dropbear|dashboard-uhttpd" | grep -v grep
```

Required invariants after reboot:

| Check | Expected |
|---|---|
| SSH 2222 | listening, key auth works |
| Dashboard 8080 | HTTP 200, SPA mount present |
| Agent 9090 | login succeeds with the deployed password |
| Stock UI 80 | HTTP 200 (must be unaffected) |
| `/etc/rc.local` bytes | unchanged from before reboot |
| `flash protect begin` count | 1 (**must not be 0**) |
| `android0/usb_op` read count | ≥ 1 (**must not be 0**) |
| boot hooks | 3 |
| `uci get zwrt_zte_dm.dm_update.dm_update_mode` | `0` |
| USB PID | `1403` (stock composition returns) |
| ADB | **gone** — this is expected, not a fault |

The unlock line is intentionally removed by `--harden` (`--remove-debug`), so
every boot returns to the stock tethering composition. To get ADB back, re-run
§3 or restore the pre-harden `rc.local`.

### 5.3 Gate

```
GATE: S4
  device booted after deployment
  all four listeners up
  rc.local byte-identical and syntactically valid
  flash-protect block intact
  FOTA auto-update = 0
```

---

## 6. FIRMWARE UPDATE — S4 → (S5 → S4)

### 6.1 Decide whether to update at all

| Consideration | Weight |
|---|---|
| The fork targets a **specific** firmware family. A newer build may differ. | high |
| FOTA wipes `/etc/rc.local` hooks **and** `/etc/dropbear`. | expected, recoverable |
| FOTA may **rotate the backup-key suffix**. If it does, re-unlocking may become impossible. | **decisive** |
| `/data` survives, so binaries, scripts and the `/data/dropbear` key copy persist. | makes recovery feasible |
| Auto-update is already off; there is no pressure to update. | lowers urgency |

**Rule: never update unless §5 has passed first.** You must have proven that you
can rebuild the boot hooks, because that is exactly what an update forces you to do.

### 6.2 Pre-update capture (do this while root still works)

```sh
OUT=/data/local/tmp/pre-update
mkdir -p "$OUT"
cp -a /etc/rc.local                 "$OUT/rc.local.before"
cp -a /data/dropbear/.              "$OUT/dropbear/"
for s in start_dropbear.sh start_dashboard.sh start_zte_agent.sh open-u60-rc-update.sh; do
    cp -a "/data/local/tmp/$s" "$OUT/"
done
{
  echo "utc: $(date -u +%FT%TZ)"
  echo "uname: $(uname -a)"
  echo "oldversion: $(uci get zwrt_zte_dm.dm_update.oldversion)"
  echo "slot: $(cat /proc/cmdline | tr ' ' '\n' | grep -i slot_suffix)"
  sha256sum /usr/bin/zte_web /usr/bin/zte_topsw_mc /usr/lib/libztemc.so 2>/dev/null
} > "$OUT/fingerprint.txt"
```

Pull `$OUT` to the host and keep it with the unlock recovery material.

### 6.3 What FOTA preserves (read the real list, do not assume)

```sh
cat /sbin/backup_fota.list          # paths the updater copies across
cat /sbin/backup_fota_ignore.list   # paths it deliberately drops
```

Observed on B31:
```
/zteoverlay/etc-upper/config/
/zteoverlay/etc-upper/manual_apn_v2.db
/zteoverlay/etc-upper/adbd.conf
/zteoverlay/etc-upper/config/zwrt_tr069
/zteoverlay/etc-upper/etc_rw/ztembb/dm/
/zteoverlay/etc-upper/wireless/mediatek/
```

`rc.local` and `dropbear` are **absent** from that list. Treat the list as
firmware-specific and re-read it on every new build.

### 6.4 Update, then rebuild

Only after §6.2. Use the stock UI's manual update. Never `abctl --set_active`.

Post-update recovery sequence (assuming ADB or SSH is available again; if the
device booted to stock without root, re-run §3 first):

```sh
# 1. SSH material — the /data copy survived
mkdir -p /etc/dropbear
cp -a /data/dropbear/. /etc/dropbear/
chmod 700 /etc/dropbear; chmod 600 /etc/dropbear/*
sh /data/local/tmp/start_dropbear.sh

# 2. rebuild the boot hooks with the repo's guarded updater
sh /data/local/tmp/open-u60-rc-update.sh \
    'sh /data/local/tmp/start_dropbear.sh' \
    'sh /data/local/tmp/start_dashboard.sh' \
    'sh /data/local/tmp/start_zte_agent.sh'
sh -n /etc/rc.local

# 3. restart the remaining services now
sh /data/local/tmp/start_zte_agent.sh
sh /data/local/tmp/start_dashboard.sh

# 4. re-disable auto-update (the update resets it)
ubus call zwrt_zte_dm set_update_mode '{"dm_update_mode":"0"}'
test "$(uci get zwrt_zte_dm.dm_update.dm_update_mode)" = 0 && echo FOTA_OFF
```

Then repeat §5.2 to prove persistence again.

### 6.5 If the suffix was rotated

Re-lock detection: `zunlock.py --dry-run` fails with
`decryption failed — wrong suffix`. The device itself still works; only future
re-unlocking is affected.

Try, in order:

1. Re-derive from the device's own web daemon. The documented source is
   `strings /usr/sbin/zte_web`; on B31 the daemon is at **`/usr/bin/zte_web`**
   and the suffix was **not** present as a contiguous string, and not
   single-byte-XOR'd. Treat this path as *unreliable*, not guaranteed.
2. Diff the pre/post binary hashes captured in §6.2 and inspect the changed
   backup-related binary (`zte_topsw_mc`, `libztemc.so` both participate).
3. Ask the community channel that supplied the original suffix, quoting the new
   firmware version. Do **not** post the key value itself publicly — see §8.

### 6.6 Gate

```
GATE: back to S4
  §5.2 table fully re-verified after the update
  firmware version recorded (before/after)
  FOTA auto-update = 0
```

---

## 7. Host-environment hazards

These are **host** bugs, not device bugs. Each one was observed on Windows and
each one silently corrupts or aborts an otherwise-correct operation.

### 7.1 CRITICAL: never inline-quote device commands through a host shell

```sh
# DANGEROUS — quoting is lost in transit on Windows/PowerShell
adb shell "grep -cF "echo 1 >" /etc/rc.local"
```

What the *device* received was `grep -cF echo 1 > /etc/rc.local` — an unquoted
redirection that **truncated `/etc/rc.local` to 0 bytes**, removing the boot
hooks and the stock flash-protect block.

**Mandatory pattern for anything that writes, or that contains shell
metacharacters (`> < | $ \` " ' ; &`):**

```
1. write the logic to a local .sh file
2. normalise it to LF endings
3. adb push <file> /data/local/tmp/<name>.sh
4. adb shell 'sh /data/local/tmp/<name>.sh'
5. rm the staged file
```

Reconnaissance reads may use `adb shell` directly, but keep them single-token
and quote-free. When in doubt, use the staged-script pattern.

### 7.2 Windows/Python portability (only if reimplementing the deploy tool)

| Symptom | Cause | Fix |
|---|---|---|
| `OSError [WinError 10093]` from `selectors.select()` | `select()` only works on sockets on Windows; the tool multiplexes pipe FDs | use `subprocess.communicate()` |
| `AttributeError: module 'os' has no attribute 'killpg'` | POSIX-only | `process.kill()` on Windows |
| `invalid literal for int()` while parsing device output | adb.exe translates the device's CRLF again → `CRCRLF`; `splitlines()` then yields empty strings | normalise `\r\n`→`\n` then strip `\r` |
| `adb: failed to read all of <tmp>: Permission denied` | `NamedTemporaryFile` keeps the file open; Windows refuses a second open without `FILE_SHARE_READ` | close the handle before calling adb |
| `sh -n` fails, no `sh` on host | Windows has no POSIX shell | run the lint on the device: push, `sh -n <path>` |
| adb selects/waits on the wrong device | bare `adb` invocation | always pass `-s <serial>` explicitly |

`scripts/deploy-components.py` is POSIX-correct as written. On Windows, prefer
running it under WSL, or wrap it (import the module unmodified, replace only
`run()`, then call `main()`) rather than editing the repo file.

### 7.3 Proxy / fake-IP DNS

A fake-IP proxy (`FlClash` and similar) resolves `github.com` to `198.18.x.x`.
`api.github.com` and `raw.githubusercontent.com` may work while `github.com`
page fetches and some tooling do not. If an authenticated `gh` session is
available it is the most reliable fetch path; otherwise use the GitHub REST API
directly via `curl` with a bearer token.

---

## 8. Red lines (from `docs/SAFETY.md`)

Violating any of these can brick the device with no software recovery.

1. **No boot hooks outside `/etc/rc.local`.** No firewall includes, no
   `init.d` service changes. A hook that stalls has no recovery interface.
2. **Never disable a `zte_topsw_daemon.conf` daemon via `init.d`.** The master
   daemon waits for all listed daemons; the device then hangs on the boot logo
   while everything else *looks* fine.
3. **Never delete the stock block that READS `usb_op`.** `sed '/usb_op/d'`
   breaks the script's syntax and kills **all** rc.local actions next boot.
   Always `sh -n /etc/rc.local` after any edit.
4. **Never write the USB composition node live.** Set it via rc.local + reboot.
   Live writes (`0` or `2` after `1`) kill the gadget until the next reboot.
5. **No partition access.** No `dd`, `mtd`, `fw_setenv`, `/dev/block`,
   QFPROM/fuse writes, no `abctl --set_active` (mixed-slot boots brick).
6. **Config backup/restore is the only sanctioned privileged path.** Always
   `--dry-run` first.
7. **FOTA auto-update stays off** (`zharden.sh` / `--harden`). A surprise update
   wipes rc.local hooks and can change the rules under the agent.
8. **`scripts/research/` stays quarantined.** It is not part of the unlock path.
9. **Never publish the backup-key suffix.** Publishing gets it rotated in the
   next firmware and breaks unlocking for everyone. Never commit it; never paste
   it into an issue.

### Rollback

```sh
sh /data/local/tmp/open-u60-transaction.sh restore <transaction-id> <identity-fingerprint>
```

The exact argv is printed by the deployer. Snapshots live in
`/data/local/tmp/open-u60-transactions/<id>/{before,present}/`. A failed
deployment restores automatically; a successful one retains the snapshot.

---

## 9. Known unknowns

State these as *unverified* rather than asserting them:

| Item | Status |
|---|---|
| Unlock on firmware newer than B31 | unverified; B28+ removed the web toggle, the backup path is firmware-agnostic in principle |
| Suffix after a ZTE rotation | **not reliably recoverable** from B31 binaries by plain-string or XOR search |
| Whether a future FOTA changes `backup_fota.list` | must be re-read every update |
| CDC-ECM host support on Windows/ARM64 | no inbox `netecm.inf`; the stock composition on B31 is **RNDIS**, not ECM, so this does not bite |

---

## 10. Quick reference

```
LAN / stock UI      http://192.168.0.1            :80, :443
OpenUI dashboard    http://192.168.0.1:8080
Agent API           http://192.168.0.1:9090       (password = chosen at deploy)
SSH                 ssh -p 2222 root@192.168.0.1  (key-only)

USB stock           VID_19D2 PID_1403  RNDIS
USB unlocked        VID_19D2 PID_1404  ADB + RNDIS + MassStorage

root check          /sys/class/android_usb/android0/usb_op   (1|2 = unlocked boot)
boot hook runner    /etc/init.d/done  (S95done)  -> sh /etc/rc.local
overlay upperdir    /zteoverlay/etc-upper_a/      (rc.local, dropbear live here)
FOTA preserve list  /sbin/backup_fota.list
FOTA toggle         uci get zwrt_zte_dm.dm_update.dm_update_mode   (must be 0)
```
