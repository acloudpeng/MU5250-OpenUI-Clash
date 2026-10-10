# U60Pro-OpenUI

简体中文 | [繁體中文](README.zh-TW.md) | [English](README.en.md)

适用于 CN B31 固件 **中兴 U60 Pro（MU5250）** 的 OpenUI。它在原厂固件之上增加三样东西：

- 网页控制台；
- 触摸屏面板；
- eSIM 配置文件管理。

本项目延续 [MU5250-OpenUI](https://github.com/dklasens/MU5250-OpenUI) 以及[致谢](#致谢)中列出的工作。

| 服务 | 地址 |
| --- | --- |
| 网页控制台 | `http://192.168.0.1:8080/` |
| 需要登录的 API | `http://192.168.0.1:9090/` |

## 设备

| **型号** | 中兴 U60 Pro（MU5250） |
| ------------ | ------------------------ |
| **硬件** | `MU5250_HW1.0` |
| **固件** | `CN_ZTE_MU5250V1.0.0B31` |

| **芯片** | 高通骁龙 X75（SDX75 / SDXPINN） |
| ----------- | ----------------------------------------- |
| **CPU** | 4 × Cortex-A55（ARMv8.2-A）@ 2.2 GHz |

以上是开发所用设备的信息，不代表兼容其他型号、硬件版本或固件。

## 功能

- **网页控制台**，包括：
  - 首页状态；
  - 信号，以及频段和小区控制；
  - Wi-Fi、路由器设置和客户端；
  - APN 配置、短信、流量统计和 TTL；
  - 电池控制、日志和系统信息。

  默认简体中文，可以切换为英文。见[控制台文档](docs/DASHBOARD.md)。
- **eSIM**：管理 SIM 卡槽里可插拔 eUICC 卡（9eSIM、5ber、eSTK.me 等）上的配置文件，功能对标 Android 上的 EasyEUICC。
  - 网页上：切换、重命名、删除配置文件，通过激活码或二维码图片下载配置文件，处理通知。
  - 触摸屏上：切换配置文件。

  基于 lpac。见 [eSIM 文档](docs/ESIM.md)。
- **触摸屏面板（DevUI）**：设备 320×480 屏幕上的菜单，语言跟随网页控制台。最多显示四个页面，例如电池与系统概览、eSIM。
  见[屏幕文档](docs/SCREEN.md)。
- **有校验的更新**：每次部署都会：
  - 核对设备身份和每个文件的校验和；
  - 保留之前安装的快照；
  - 任何一步失败时恢复原样。

  在 Windows 上，双击一次就能把设备更新到某个发布版本。

## DevUI 快速上手

1. 在控制台侧栏打开 **DevUI**，点击 **打开 DevUI 屏幕**。
2. 短按电源键让屏幕休眠或唤醒。
3. 想从原厂界面直接进入：打开 **连按四次打开 DevUI** 并保存。之后在 2.5 秒内短按四次电源键，每两次之间不超过 0.7 秒。
4. 亮度、闲置返回时间（30–600 秒）、启动页和菜单功能都在同一个网页上设置。保存后显示和菜单立即生效，启动页在下次打开 DevUI 时生效。

新安装默认不开启连按快捷方式。原厂的长按功能照常可用，屏幕上始终有 **原厂界面** 和 **返回**。退出 DevUI 不影响网络。
见 [DevUI 说明](docs/SCREEN.md)。

## 安装和更新

下面的步骤用于更新已经装好 OpenUI、并在 2222 端口开着 SSH 的设备。第一次解锁设备、安装 OpenUI 时，先读
[docs/SAFETY.md](docs/SAFETY.md)，再按 [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md) 操作（英文）。

### Windows：一步更新

在 Windows 10 或 11 上，双击仓库里的 `deploy-windows.cmd`。它把设备更新到与仓库版本对应的发布版本，例如版本 1.0.0 对应发布 v1.0。它会：

1. 查找 Python 3.10 以上版本和 OpenSSH 客户端；缺 Python 时提示用 winget 安装。
2. 从 GitHub 下载发布文件，并按 `sha256sums.txt` 校验。也可以用 `-AssetsDir` 指定你自己下载好文件的文件夹。
3. 下载地理数据库（`scripts/prepare-geodata.py`）。
4. 先用 `scripts/deploy-components.py` 试运行，询问你之后再正式部署。设备保留原来的 agent 密码和 PIN，已经装好的组件会跳过。
5. 屏幕程序有变化时，运行触摸屏检查（`scripts/verify-screen.py`，约三分钟）。

第一次运行会询问设备地址和 SSH 私钥（默认 `192.168.0.1` 和 `~\.ssh\id_ed25519`），之后记在 `build\windows-deploy.json` 里。
查看全部选项：`Get-Help .\scripts\windows\deploy.ps1 -Detailed`。下面的 Python 工具也可以直接在 Windows 上运行。

### 发布版本

每个[发布版本](https://github.com/acloudpeng/U60Pro-OpenUI/releases)都列出了文件及其 SHA-256 校验和。在同一版本的仓库里，用
`scripts/deploy-components.py` 部署这些文件，方法见下文。

发布页附带的桌面安装器来自上游 OpenUI，会下载上游的包并覆盖本项目新增的内容，不要用于本项目。

### 从源码构建

需要：
- Rust，带 `aarch64-unknown-linux-musl` 目标，以及 `cargo-zigbuild`；
- Zig 0.14.1；
- Python 3；
- Node.js `^20.19.0 || >=22.12.0` 和 npm。

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

屏幕程序的构建会下载固定校验和的 litehtml、FreeType 和 GNU Unifont。lpac 的构建会下载固定校验和的 lpac v2.3.0 源码，并核对结果的校验和。
这些命令都不会连接或改动你的设备。

### 用 deploy-components.py 部署

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

- `--keep-credentials`：保留设备上现有的 agent 密码和 PIN。不加时，脚本会询问密码；PIN 通过环境变量 `ZTE_AGENT_PIN` 提供，不提供就会关闭 PIN 登录。
- `--skip-unchanged`：跳过设备上已有、校验和相同的组件。
- 先看试运行的结果，没问题再去掉 `--dry-run` 重新运行。部署走 OpenUI 的快照和回滚流程。
- 替换屏幕程序之前，先让屏幕回到原厂界面。

安装后，在使用网页上的屏幕入口之前，先运行有人值守的恢复检查：

```sh
python3 scripts/verify-screen.py --gateway 192.168.0.1 \
  --port 2222 --ssh-key '/path/to/id_ed25519'
```

检查期间会临时接管屏幕，测试正常、闲置、崩溃和卡住时的恢复。其他组件和选项见 [docs/SCREEN.md](docs/SCREEN.md) 和
[docs/DEPLOYMENT.md](docs/DEPLOYMENT.md)。

## 本地演示和检查

分别在两个终端里运行：

```sh
python3 web-app/tools/mock_agent.py --port 19090
```

```sh
cd web-app
npm ci
npm run demo
```

打开 Vite 给出的地址，用任意非空密码登录。两个开发服务器都只监听本机。模拟服务的数据只在内存里，不会连接设备。

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

## 文档

文档为英文：

- [部署](docs/DEPLOYMENT.md)：解锁、安装、更新和恢复
- [安全](docs/SAFETY.md)：保证设备能恢复的规则
- [控制台](docs/DASHBOARD.md)、[触摸屏面板](docs/SCREEN.md)和 [eSIM](docs/ESIM.md)
- [Agent API](docs/AGENT.md)
- [操作手册](docs/OPERATIONS.md)：解锁、部署、验证和固件更新的逐步操作

## 致谢

- [dklasens/MU5250-OpenUI](https://github.com/dklasens/MU5250-OpenUI)：OpenUI 的后端、控制台、安装器和部署基础。
- [ZweiChen](https://github.com/czwstc)：本项目延续的分支，包括中英文触摸屏菜单。
- [jesther-ai/open-u60-pro](https://github.com/jesther-ai/open-u60-pro)：最初的 U60 开源项目，以及 OpenUI 所依赖的设备访问工作。
- [amenekowo/mu5250_tweaking](https://github.com/amenekowo/mu5250_tweaking)：MU5250 硬件研究，以及触摸屏面板使用的 Atomic DRM 示例。
- [33333s/u60pro-devui](https://github.com/33333s/u60pro-devui)：DevUI 菜单使用的 HTML/CSS RGB565 渲染器。
- [scoltzero/u60pro-devui-remix](https://github.com/scoltzero/u60pro-devui-remix)：触摸屏菜单中设备控制的参考。
- [estkme-group/lpac](https://github.com/estkme-group/lpac) 和 [OpenEUICC](https://gitea.angry.im/PeterCxy/OpenEUICC)：
  本项目运行的 eSIM LPA，以及 eSIM 页面所参照的应用。
- [SadykovIV/ZTE_U60Pro](https://github.com/SadykovIV/ZTE_U60Pro)：在 B31 上验证过的 eSIM 切换流程。

也感谢 litehtml、FreeType 和 GNU Unifont。原作者和许可证声明都已保留。本仓库不包含中兴固件、私人配置或原始参考压缩包。

## 版权和许可证

各部分的版权归各自作者：
- OpenUI：David Klasens 和 Jesther Silvestre；
- 本项目延续的分支：ZweiChen；
- 本仓库新增的内容：acloudpeng。

| 组件 | 许可证 |
| --- | --- |
| OpenUI agent、控制台和通用工具 | [MIT](LICENSE)，保留 David Klasens 和 Jesther Silvestre 的声明 |
| 单独构建的屏幕程序和改编的 DRM 代码 | [GPL-3.0-or-later](screen/LICENSE) |
| 内嵌的 GNU Unifont OTF 字体 | [SIL Open Font License 1.1](screen/FONT-LICENSE) |
| lpac（单独的 eSIM 程序） | [AGPL-3.0-only](licenses/lpac/)，其中 libeuicc 为 LGPL-2.1-only，cJSON 为 MIT |
| 依赖和下载的工具 | 各自上游的许可证 |

见 [NOTICE](NOTICE)、[屏幕程序声明](screen/NOTICE)和[保留的上游许可证](licenses/)。根目录的 MIT 许可证不会改变 GPL 屏幕代码或第三方字体的许可。
分发 GPL 屏幕程序时，必须附带对应的源码和相关声明；屏幕程序的源码和构建脚本都在本仓库里。
