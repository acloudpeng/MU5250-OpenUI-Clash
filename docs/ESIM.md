# eSIM — profiles on a removable eUICC

OpenUI manages the profiles on a removable eUICC card (9eSIM, 5ber, eSTK.me and
similar) in the U60 Pro's SIM tray, as [EasyEUICC](https://gitea.angry.im/PeterCxy/OpenEUICC)
does on Android: **Modem → eSIM** in the web page, and an **eSIM** page on the touchscreen.

EasyEUICC itself is an Android app; it reaches the SIM through Android's telephony
framework, which the modem's firmware does not have. Its LPA core is
[lpac](https://github.com/estkme-group/lpac), and that runs here: the agent runs
lpac and carries its card commands and SM-DP+ requests.

| | Web page | Screen |
|---|---|---|
| Card details: EID, SGP.22 version, eUICC firmware, free space, default SM-DP+ | ✓ | card type and profile count |
| Profile list with name, provider, ICCID, class and state | ✓ | names, last four ICCID digits |
| Switch (enable), disable | ✓ | switch |
| Rename (nickname), delete (type the name to confirm) | ✓ | — |
| Download: `LPA:1$…` code, QR image (file or pasted screenshot), or address and matching ID; confirmation code; optional IMEI | ✓ | — |
| Notifications: list, send, remove | ✓ | — |
| Restart after a switch the firmware did not take up | ✓ | ✓ |

Requirements: a removable eUICC in the SIM tray and lpac deployed (`--lpac`, below).
An ordinary SIM card is recognised and reported; nothing can be downloaded to it. A
built-in ZTE eSIM, on units that have one, is not supported.

## How it works

- **lpac** v2.3.0, built by `scripts/prepare-lpac.py`: a static aarch64 binary with only
  lpac's stdio APDU and HTTP drivers, from the SHA-256-pinned source archive, the
  patches in `scripts/lpac-patches/` and Zig 0.14.1; the output checksum is pinned
  (reproducible). Later lpac versions load drivers with `dlopen()`, which a static
  binary cannot. The patches backport seven upstream fixes (the stdio driver could not
  work at all in v2.3.0) and add two of OpenUI's own: cleanup variables that were freed
  uninitialised when a command failed (lpac crashed and left its logical channel
  open), and reading stdio replies with `getline()` instead of one character at a
  time (seconds for a profile package on the modem). Installed as `/data/bin/lpac`.
- **The agent** (`agent/src/esim.rs`, `lpac.rs`, `qmi.rs`) starts lpac for each
  command and answers its requests (`lpac.rs`):
  - APDUs go to the modem's **QMI UIM service over QRTR** (`AF_QIPCRTR`), as their own
    QMI client beside the firmware's: Open Logical Channel (0x42) to the ISD-R, Send
    APDU (0x3B), Logical Channel (0x3F) to close. Channels lpac leaves open are closed
    after it exits. No AT commands, no libqmi or glib on the device.
  - SM-DP+ requests go through `/usr/bin/curl`, HTTPS only, with the address and body
    in curl's config on stdin. TLS is verified against the GSMA production CIs that
    SM-DP+ certificates chain to (GSM Association RSP2 Root CI1, OISTE GSMA CI G1,
    Entrust eSIM CA; `agent/certs/`) plus the device's
    `/etc/ssl/certs/ca-certificates.crt`. Requests go through Clash while it runs,
    otherwise directly; the download form can force either, or skip certificate
    checks for an SM-DP+ the router does not trust (the activation code is then not
    protected in transit).
- Reading the card: Get Card Status, then opening a channel to the ISD-R; status
  `6A82` means an ordinary SIM. Then `lpac chip info`, `profile list` and
  `notification list`. Reading changes nothing; the page and the screen read the card
  when opened until it has been read once.
- One eSIM job runs at a time on its own thread and holds a wake lock that expires on
  its own after ten minutes. Scratch files (the CA bundle, a reply body) are in
  `/data/local/tmp/openui-esim` (0700). Nothing is persisted: an agent restart (a
  reboot) forgets the last read.

### Switching

1. `lpac profile enable <iccid> 0` — without the card's own REFRESH; the card is read
   back to see whether it switched, since a timed-out command may still have.
2. The modem reads the card again, the sequence another project verified on B31
   ([SadykovIV/ZTE_U60Pro](https://github.com/SadykovIV/ZTE_U60Pro), 9eSIM V0): radio
   to low power (QMI DMS, waiting up to 20 s for it), SIM power off and on (QMI UIM,
   slot 1), radio online (waiting up to 20 s). A radio that was not online is left
   alone; a failure always powers the card back on and sets the radio online again.
3. Waits up to 30 s for the modem to report the new ICCID in the active slot, up to
   30 s for the USIM to be ready (a profile with a PIN stays unready), then up to 60 s
   for the firmware's `zwrt_zte_mdm.api get_sim_info` to report it.

If the firmware keeps the previous profile, the page and the screen offer a
**restart** — never automatically, and firmware daemons are never restarted
(SAFETY.md). A switch takes the mobile connection down for 10–30 s; switching to a
profile without data from a remote session loses access. Some cards (eSTK.me) answer
`catBusy` to quick repeated switches; a restart or reinserting the card clears it.

Disabling leaves the card without an enabled profile, and the router without mobile
service until one is enabled.

### Notifications

A download or delete queues a notification for the SM-DP+; OpenUI sends those at once,
as EasyEUICC does by default, and keeps any that fail. That includes the error result
of a download the card refused (for example, an ICCID already on the card), so the
SM-DP+ learns the download failed. Switching queues enable and
disable notifications, which stay on the card until they are sent or removed from the
notification list. Activation codes are often single-use: after a failed download,
read the card before trying again.

## Interfaces

- `GET /api/esim`: `installed`, the last read (`view`: card, EID, details, profiles,
  notifications, the firmware's ICCID and state, a generation number), the running or
  last `job` (kind, state, step, error, result) and `reboot_needed`.
- `POST /api/esim` with `{"action": …}`: `scan`; `enable` / `disable` with `iccid`;
  `nickname` with `iccid` and `nickname` (≤ 64 bytes); `download` with
  `activation_code` or `smdp` + `matching_id`, and optional `confirmation_code`
  (required when the code's fifth field is 1), `imei`, `route` (`auto`, `proxy`,
  `direct`) and `insecure`; `notifications` with
  `operation` (`process` or `remove`) and `seq`. Each starts a job and answers at once
  (409 while one runs, 503 without lpac).
- `POST /api/esim/delete` with `iccid` needs `X-Confirm: true`; only a disabled
  profile can be deleted.
- Screen (docs/SCREEN.md): `Q` and `q` records (card type, generation, job, restart
  flag; profile names, classes and the last four ICCID digits — no EID), and the
  actions `esim scan`, `esim enable <generation> <index>` and `esim reboot` (only while
  a switch waits for one).

## Deploy

```sh
ZIG=/path/to/zig python3 scripts/prepare-lpac.py   # build/lpac/lpac, checksum-verified
python3 scripts/deploy-components.py --gateway 192.168.0.1 --ssh-key /path/to/id_ed25519 \
  --agent build/zte-agent --dashboard build/dashboard-dist.tar.gz --lpac build/lpac/lpac --dry-run
```

`lpac` is also a release asset (built by `.github/workflows/lpac.yml`). Deployment
checks that it is the pinned build, runs `lpac version` from a staged copy, and keeps
`/data/bin/lpac` and its licence texts (`/data/local/tmp/openui-lpac-licenses`) in the
rollback snapshot.

## Verification

- Agent unit tests: QMI and QRTR encoding, slot and card status parsing, the stdio
  bridge (with a stand-in lpac), activation codes, curl options, the switch sequence
  with a simulated modem, screen records and actions. Screen preview self-tests cover
  the eSIM page in both languages.
- `tests/lpac_stdio.py` drives the built lpac (arm64 under qemu in CI): request order,
  the error path closing its channel, a 2 MB reply.
- End-to-end on the simulated router: the real agent and lpac (x86_64) against a
  simulated modem and eUICC over the debug-build Unix-socket QMI transport
  (`OPENUI_QMI_SOCKET_DIR`), and an HTTPS SM-DP+: reading, renaming, switching (and the
  order of radio and SIM power steps), a firmware that does not follow, catBusy,
  deleting, downloads (accepted, refused, with a confirmation code, a duplicate ICCID),
  notifications, ordinary SIM and empty tray, with no logical channel left open; and
  the web tab in a browser.

Still to verify on a device: QRTR access to the UIM service under the B31 firmware,
the switch sequence and whether the firmware takes up a new profile without a
restart, and downloads from real SM-DP+ servers. With an ordinary SIM in the tray,
**Read the card** checks the first of these: it should report an ordinary SIM card.

## Licences

lpac is AGPL-3.0-only (with libeuicc LGPL-2.1-only and cJSON MIT); see
`licenses/lpac/`. It is a separate program the MIT-licensed agent starts. The source
archive, patches and build script in this repository are its corresponding source.
The QR decoder in the web page is jsQR (Apache-2.0), shipped with its licence in
`licenses/jsQR-LICENSE.txt` of the dashboard.
