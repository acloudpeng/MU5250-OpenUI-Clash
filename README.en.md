# U60Pro-OpenUI

[简体中文](README.md) | [繁體中文](README.zh-TW.md) | English

OpenUI for the **ZTE U60 Pro (MU5250)** on CN B31 firmware. It adds three things to
the stock firmware:

- a web dashboard;
- a touchscreen panel;
- eSIM profile management.

It continues [MU5250-OpenUI](https://github.com/dklasens/MU5250-OpenUI) and the work
listed under [Acknowledgements](#acknowledgements).

| Service | Address |
| --- | --- |
| Dashboard | `http://192.168.0.1:8080/` |
| Authenticated API | `http://192.168.0.1:9090/` |

## Device

| **Model** | ZTE U60 Pro (MU5250) |
| ------------ | ------------------------ |
| **Hardware** | `MU5250_HW1.0` |
| **Firmware** | `CN_ZTE_MU5250V1.0.0B31` |

| **Chipset** | Qualcomm Snapdragon X75 (SDX75 / SDXPINN) |
| ----------- | ----------------------------------------- |
| **CPU** | 4x Cortex-A55 (ARMv8.2-A) @ 2.2 GHz |

These are the details of the device the project is developed on. They do not
establish compatibility with other models, hardware revisions or firmware.

## Features

- **Dashboard.** It covers:
  - home status;
  - signal, with band and cell controls;
  - Wi-Fi and router settings, and clients;
  - APN profiles, SMS, data usage and TTL;
  - battery controls, logging and system information.

  It is in Simplified Chinese by default, with an English switch. See the
  [dashboard documentation](docs/DASHBOARD.md).
- **eSIM.** Manage the profiles on a removable eUICC card (9eSIM, 5ber, eSTK.me and
  similar) in the SIM tray, as EasyEUICC does on Android.
  - On the web page: switch, rename and delete profiles, download them from an
    activation code or a QR image, and handle notifications.
  - On the touchscreen: switch profiles.

  It uses lpac. See the [eSIM documentation](docs/ESIM.md).
- **Wi-Fi relay.** Join an existing Wi-Fi network (home, hotel, office) and use it as the
  internet connection while the device's own hotspot keeps running, saving mobile data.
  When the upstream network drops or has no internet, traffic returns to the cellular link
  and switches back once it recovers. Several networks can be saved and are tried in
  order; turn it on and off on the webpage or the touchscreen.
  See the [Wi-Fi relay documentation](docs/WIFI-UPLINK.md).
- **Touchscreen panel (DevUI).** A menu on the device's 320×480 screen, in the
  dashboard's language. It shows up to nine pages (five or more as a 3×3 toolbox grid),
  such as the battery and system overview, eSIM, Wi-Fi, signal and charging. See the [screen documentation](docs/SCREEN.md).
- **Checked updates.** Each deployment does the following:
  - verifies the device and every file's checksum;
  - keeps a snapshot of the previous installation;
  - restores it if a step fails.

  On Windows, a double-click updates the device to a release.

## DevUI quick start

1. Open **DevUI** in the dashboard sidebar and select **Open DevUI screen**.
2. To enter on the device, hold two fingers still on the lit stock UI for 2 seconds,
   where the stock UI has no button. Two more ways are switched on in the webpage and saved:
   - **Open DevUI at startup**: opens once the stock UI has finished starting, never
     while the device charges switched off;
   - **Press four times to open DevUI**: four short power-button presses within
     2.5 seconds, with gaps no longer than 0.7 seconds.
3. Short-press the power button to sleep or wake the display; hold it for about 1.5
   seconds for the power menu: shut down, restart or return to the stock UI (shutting
   down and restarting ask once more). After the idle time
   (two minutes by default) the screen turns off and DevUI stays; you can choose to
   return to the stock UI instead.
4. Set brightness, idle time (30–600 seconds), what happens when idle, the startup
   page and visible menu features on the same webpage. Saving applies display and
   menu changes live; the startup page applies the next time DevUI opens.

The two-finger hold is on by default; opening at startup and the four-press shortcut
are off. The stock lock screen does not stop the two-finger hold or the four presses.
Stock UI and Back (top left) are always accessible. Leaving DevUI does not affect the network. See [DevUI details](docs/SCREEN.md).

## Install and update

The steps below update a modem that already runs OpenUI, with SSH on port 2222. To
unlock a device and install OpenUI for the first time, read
[docs/SAFETY.md](docs/SAFETY.md), then [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md).

### Windows: one step

On Windows 10 or 11, double-click `deploy-windows.cmd` in a checkout of this
repository. It updates the modem to the release that matches the checkout; for
example, version 1.1.0 deploys v1.1. It does the following:

1. Finds Python 3.10 or newer and the OpenSSH client. If Python is missing, it offers
   to install it with winget.
2. Downloads the release files from GitHub and checks them against `sha256sums.txt`.
   To use a folder you downloaded yourself instead, pass `-AssetsDir`.
3. Downloads the geo databases (`scripts/prepare-geodata.py`).
4. Runs `scripts/deploy-components.py` as a dry run, asks, then deploys. The modem
   keeps its agent password and PIN, and components it already has are left out.
5. If the screen program changed, runs the touchscreen check
   (`scripts/verify-screen.py`, about three minutes).

The first run asks for the modem address and SSH key (default `192.168.0.1` and
`~\.ssh\id_ed25519`). The script remembers them in `build\windows-deploy.json`. For the
options, run `Get-Help .\scripts\windows\deploy.ps1 -Detailed`. The Python tools
below also run directly on Windows.

### Releases

Each [release](https://github.com/acloudpeng/U60Pro-OpenUI/releases) lists its files
and their SHA-256 checksums. Deploy them with `scripts/deploy-components.py` from a
checkout of the same version, as shown below.

The desktop installers attached to releases come from upstream OpenUI. They download
upstream packages and would overwrite this project's additions, so do not use them
here.

### Build from this repository

Requires the following:
- Rust with the `aarch64-unknown-linux-musl` target, and `cargo-zigbuild`;
- Zig 0.14.1;
- Python 3;
- Node.js `^20.19.0 || >=22.12.0` with npm.

```sh
cargo zigbuild --locked --release -p zte-agent --target aarch64-unknown-linux-musl
cd web-app
npm ci
npm run build
cd ..
mkdir -p build
tar czf build/dashboard-dist.tar.gz -C web-app/dist .
ZIG=/path/to/zig sh screen/build.sh
ZIG=/path/to/zig python3 scripts/prepare-lpac.py
```

The screen build fetches checksum-pinned litehtml, FreeType and GNU Unifont. The lpac
build fetches the checksum-pinned lpac v2.3.0 source and verifies the checksum of the
result. None of these commands contacts or changes your modem.

### Deploy with deploy-components.py

```sh
python3 scripts/deploy-components.py \
  --gateway 192.168.0.1 \
  --ssh-key '/path/to/id_ed25519' \
  --agent target/aarch64-unknown-linux-musl/release/zte-agent \
  --dashboard build/dashboard-dist.tar.gz \
  --screen build/screen/openui-screen \
  --lpac build/lpac/lpac \
  --keep-credentials \
  --dry-run
```

- `--keep-credentials` keeps the agent password and PIN that the modem already has.
  Without it, the script asks for the password. Pass the PIN in `ZTE_AGENT_PIN`; an
  omitted PIN turns PIN login off.
- `--skip-unchanged` leaves out components already installed at the same checksum.
- Review the dry-run result, then repeat without `--dry-run`. The deployment goes
  through OpenUI's snapshot and rollback flow.
- Restore the stock screen before replacing the screen program.

After installation, run the supervised recovery check before using the webpage's
screen entry:

```sh
python3 scripts/verify-screen.py --gateway 192.168.0.1 \
  --port 2222 --ssh-key '/path/to/id_ed25519'
```

The check temporarily takes over the display, and tests normal, idle, crash and
stalled-panel recovery. See [docs/SCREEN.md](docs/SCREEN.md) and
[docs/DEPLOYMENT.md](docs/DEPLOYMENT.md) for the other components and options.

## Local demo and checks

Run these in two terminals:

```sh
python3 web-app/tools/mock_agent.py --port 19090
```

```sh
cd web-app
npm ci
npm run demo
```

Open the Vite URL and sign in with any nonempty demo password. Both development
servers stay on loopback. The mock holds data only in memory and does not contact
the device.

```sh
cargo test --locked -p zte-agent -p process-runner
python3 -m unittest discover -s tests -v
python3 scripts/check-api-contract.py
cd web-app
npm run build
npm run lint
npm test
cd ..
sh screen/build.sh --preview
```

## Documentation

- [Deployment](docs/DEPLOYMENT.md): unlock, install, update and recovery
- [Safety](docs/SAFETY.md): rules that keep the device recoverable
- [Dashboard](docs/DASHBOARD.md), [touchscreen panel](docs/SCREEN.md), [eSIM](docs/ESIM.md) and [Wi-Fi relay](docs/WIFI-UPLINK.md)
- [Agent API](docs/AGENT.md)
- [Operations](docs/OPERATIONS.md): step-by-step unlock, deployment, verification and
  firmware updates

## Acknowledgements

- [dklasens/MU5250-OpenUI](https://github.com/dklasens/MU5250-OpenUI): the OpenUI
  backend, dashboard, installer and deployment foundation.
- [ZweiChen](https://github.com/czwstc): the fork this project continues, including the
  bilingual touchscreen menu.
- [jesther-ai/open-u60-pro](https://github.com/jesther-ai/open-u60-pro): the original
  open U60 project and the device access work that OpenUI is based on.
- [amenekowo/mu5250_tweaking](https://github.com/amenekowo/mu5250_tweaking): MU5250
  hardware research, and the Atomic DRM example used for the touchscreen panel.
- [33333s/u60pro-devui](https://github.com/33333s/u60pro-devui): the HTML/CSS RGB565
  renderer used by the DevUI menu.
- [scoltzero/u60pro-devui-remix](https://github.com/scoltzero/u60pro-devui-remix): a
  reference for device controls in the touchscreen menu.
- [estkme-group/lpac](https://github.com/estkme-group/lpac) and
  [OpenEUICC](https://gitea.angry.im/PeterCxy/OpenEUICC): the eSIM LPA this project
  runs, and the app whose features its eSIM page follows.
- [SadykovIV/ZTE_U60Pro](https://github.com/SadykovIV/ZTE_U60Pro): the eSIM switching
  sequence verified on B31.

Thanks also to litehtml, FreeType and GNU Unifont. Original author and license notices
are preserved. This repository includes no ZTE firmware, private configuration or
original reference archive.

## Copyright and licenses

Each part keeps its authors' copyright:
- OpenUI: David Klasens and Jesther Silvestre;
- the fork this project continues: ZweiChen;
- this repository's additions: acloudpeng.

| Component | License |
| --- | --- |
| OpenUI agent, dashboard and general tooling | [MIT](LICENSE), preserving David Klasens and Jesther Silvestre notices |
| Separate native screen program and adapted DRM code | [GPL-3.0-or-later](screen/LICENSE) |
| Embedded GNU Unifont OTF | [SIL Open Font License 1.1](screen/FONT-LICENSE) |
| lpac, the separate eSIM program | [AGPL-3.0-only](licenses/lpac/), with libeuicc LGPL-2.1-only and cJSON MIT |
| Dependencies and downloaded tools | Their respective upstream licenses |

See [NOTICE](NOTICE), the [screen notices](screen/NOTICE) and the [preserved upstream
licenses](licenses/). The root MIT license does not relicense the GPL screen code or
third-party fonts. Distributing the GPL screen executable requires its corresponding
source and the applicable notices; the screen source and build scripts are in this
repository.
