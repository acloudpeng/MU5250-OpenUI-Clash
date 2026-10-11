# U60Pro-OpenUI

[简体中文](README.md) | 繁體中文 | [English](README.en.md)

適用於 CN B31 韌體 **中興 U60 Pro（MU5250）** 的 OpenUI。它在原廠韌體之上增加三樣東西：

- 網頁控制台；
- 觸控螢幕面板；
- eSIM 設定檔管理。

本專案延續 [MU5250-OpenUI](https://github.com/dklasens/MU5250-OpenUI) 以及[致謝](#致謝)中列出的成果。

| 服務 | 位址 |
| --- | --- |
| 網頁控制台 | `http://192.168.0.1:8080/` |
| 需要登入的 API | `http://192.168.0.1:9090/` |

## 裝置

| **型號** | 中興 U60 Pro（MU5250） |
| ------------ | ------------------------ |
| **硬體** | `MU5250_HW1.0` |
| **韌體** | `CN_ZTE_MU5250V1.0.0B31` |

| **晶片** | 高通 Snapdragon X75（SDX75 / SDXPINN） |
| ----------- | ----------------------------------------- |
| **CPU** | 4 × Cortex-A55（ARMv8.2-A）@ 2.2 GHz |

以上是開發所用裝置的資訊，不代表相容其他型號、硬體版本或韌體。

## 功能

- **網頁控制台**，包括：
  - 首頁狀態；
  - 訊號，以及頻段和基地台控制；
  - Wi-Fi、路由器設定和用戶端；
  - APN 設定、簡訊、流量統計和 TTL；
  - 電池控制、記錄檔和系統資訊。

  介面提供簡體中文（預設）和英文。請見[控制台文件](docs/DASHBOARD.md)。
- **eSIM**：管理 SIM 卡槽中可插拔 eUICC 卡（9eSIM、5ber、eSTK.me 等）上的設定檔，功能對標 Android 上的 EasyEUICC。
  - 網頁上：切換、重新命名、刪除設定檔，透過啟用碼或 QR 碼圖片下載設定檔，處理通知。
  - 觸控螢幕上：切換設定檔。

  基於 lpac。請見 [eSIM 文件](docs/ESIM.md)。
- **觸控螢幕面板（DevUI）**：裝置 320×480 螢幕上的選單，語言跟隨網頁控制台。最多顯示四個頁面，例如電池與系統概覽、eSIM。
  請見[螢幕文件](docs/SCREEN.md)。
- **有校驗的更新**：每次部署都會：
  - 核對裝置身分和每個檔案的校驗碼；
  - 保留先前安裝的快照；
  - 任何一步失敗時還原。

  在 Windows 上，按兩下就能把裝置更新到某個發行版本。

## DevUI 快速上手

網頁控制台的介面只有簡體中文和英文，下面的按鈕名稱照畫面上的寫，括號裡是英文介面的名稱。

1. 在控制台側欄開啟 **DevUI**，點選 **打开 DevUI 屏幕**（Open DevUI screen）。
2. 在裝置上直接進入：原廠介面亮著時，用兩根手指按住螢幕不動 2 秒，手指放在原廠介面沒有按鈕的地方。
   另外兩種方式要在網頁上開啟並儲存：
   - **开机自动打开 DevUI**（Open DevUI at startup）：開機後等原廠介面啟動完成再開啟，關機充電時不會開啟；
   - **连按四次打开 DevUI**（Press four times to open DevUI）：2.5 秒內短按四次電源鍵，每兩次之間不超過 0.7 秒。
3. 短按電源鍵讓螢幕休眠或喚醒。閒置到設定的時間（預設 2 分鐘）後螢幕熄滅，DevUI 保留；也可以改成返回原廠介面。
4. 亮度、閒置時間（30–600 秒）、閒置後的操作、啟動頁和選單功能都在同一個網頁上設定。儲存後顯示和選單立即生效，啟動頁在下次開啟 DevUI 時生效。

雙指按住預設開啟，開機開啟和連按捷徑預設關閉。原廠鎖定畫面擋不住雙指按住和連按：鎖定時這樣操作，同樣會開啟 DevUI。
原廠的長按功能照常可用，螢幕上始終有 **原厂界面**（Stock UI）和 **返回**（Back）。退出 DevUI 不影響網路。請見 [DevUI 說明](docs/SCREEN.md)。

## 安裝和更新

下面的步驟用於更新已經裝好 OpenUI、並在 2222 連接埠開著 SSH 的裝置。第一次解鎖裝置、安裝 OpenUI 時，請先讀
[docs/SAFETY.md](docs/SAFETY.md)，再依照 [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md) 操作（英文）。

### Windows：一步更新

在 Windows 10 或 11 上，按兩下儲存庫裡的 `deploy-windows.cmd`。它會把裝置更新到與儲存庫版本對應的發行版本，例如版本 1.1.0 對應發行版 v1.1。
它會：

1. 尋找 Python 3.10 以上版本和 OpenSSH 用戶端；缺少 Python 時提示用 winget 安裝。
2. 從 GitHub 下載發行檔案，並依 `sha256sums.txt` 校驗。也可以用 `-AssetsDir` 指定你自己下載好檔案的資料夾。
3. 下載地理資料庫（`scripts/prepare-geodata.py`）。
4. 先用 `scripts/deploy-components.py` 試執行，詢問你之後再正式部署。裝置保留原來的 agent 密碼和 PIN，已經裝好的元件會略過。
5. 螢幕程式有變更時，執行觸控螢幕檢查（`scripts/verify-screen.py`，約三分鐘）。

第一次執行會詢問裝置位址和 SSH 私密金鑰（預設 `192.168.0.1` 和 `~\.ssh\id_ed25519`），之後記在 `build\windows-deploy.json` 裡。
查看所有選項：`Get-Help .\scripts\windows\deploy.ps1 -Detailed`。下面的 Python 工具也能直接在 Windows 上執行。

### 發行版本

每個[發行版本](https://github.com/acloudpeng/U60Pro-OpenUI/releases)都列出了檔案及其 SHA-256 校驗碼。在同一版本的儲存庫裡，用
`scripts/deploy-components.py` 部署這些檔案，方法見下文。

發行頁附帶的桌面安裝程式來自上游 OpenUI，會下載上游的套件並覆蓋本專案新增的內容，請勿用於本專案。

### 從原始碼建置

需要：
- Rust，含 `aarch64-unknown-linux-musl` 目標，以及 `cargo-zigbuild`；
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

螢幕程式的建置會下載固定校驗碼的 litehtml、FreeType 和 GNU Unifont。lpac 的建置會下載固定校驗碼的 lpac v2.3.0 原始碼，並核對結果的校驗碼。
這些指令都不會連線或改動你的裝置。

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

- `--keep-credentials`：保留裝置上現有的 agent 密碼和 PIN。不加時，指令碼會詢問密碼；PIN 透過環境變數 `ZTE_AGENT_PIN` 提供，不提供就會關閉 PIN 登入。
- `--skip-unchanged`：略過裝置上已有、校驗碼相同的元件。
- 先看試執行的結果，沒問題再去掉 `--dry-run` 重新執行。部署會經過 OpenUI 的快照和回復流程。
- 替換螢幕程式之前，先讓螢幕回到原廠介面。

安裝後，在使用網頁上的螢幕入口之前，先執行有人看管的復原檢查：

```sh
python3 scripts/verify-screen.py --gateway 192.168.0.1 \
  --port 2222 --ssh-key '/path/to/id_ed25519'
```

檢查期間會暫時接管螢幕，測試正常、閒置、當機和卡住時的復原。其他元件和選項請見 [docs/SCREEN.md](docs/SCREEN.md) 和
[docs/DEPLOYMENT.md](docs/DEPLOYMENT.md)。

## 本機示範和檢查

分別在兩個終端機裡執行：

```sh
python3 web-app/tools/mock_agent.py --port 19090
```

```sh
cd web-app
npm ci
npm run demo
```

開啟 Vite 給出的網址，用任意非空白密碼登入。兩個開發伺服器都只監聽本機。模擬服務的資料只存在記憶體中，不會連線到裝置。

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

## 文件

文件為英文：

- [部署](docs/DEPLOYMENT.md)：解鎖、安裝、更新和復原
- [安全](docs/SAFETY.md)：確保裝置能夠復原的規則
- [控制台](docs/DASHBOARD.md)、[觸控螢幕面板](docs/SCREEN.md)和 [eSIM](docs/ESIM.md)
- [Agent API](docs/AGENT.md)
- [操作手冊](docs/OPERATIONS.md)：解鎖、部署、驗證和韌體更新的逐步操作

## 致謝

- [dklasens/MU5250-OpenUI](https://github.com/dklasens/MU5250-OpenUI)：OpenUI 的後端、控制台、安裝程式和部署基礎。
- [ZweiChen](https://github.com/czwstc)：本專案延續的分支，包括中英文觸控螢幕選單。
- [jesther-ai/open-u60-pro](https://github.com/jesther-ai/open-u60-pro)：最初的 U60 開源專案，以及 OpenUI 所依賴的裝置存取工作。
- [amenekowo/mu5250_tweaking](https://github.com/amenekowo/mu5250_tweaking)：MU5250 硬體研究，以及觸控螢幕面板使用的 Atomic DRM 範例。
- [33333s/u60pro-devui](https://github.com/33333s/u60pro-devui)：DevUI 選單使用的 HTML/CSS RGB565 繪製器。
- [scoltzero/u60pro-devui-remix](https://github.com/scoltzero/u60pro-devui-remix)：觸控螢幕選單中裝置控制的參考。
- [estkme-group/lpac](https://github.com/estkme-group/lpac) 和 [OpenEUICC](https://gitea.angry.im/PeterCxy/OpenEUICC)：
  本專案執行的 eSIM LPA，以及 eSIM 頁面所參照的應用程式。
- [SadykovIV/ZTE_U60Pro](https://github.com/SadykovIV/ZTE_U60Pro)：在 B31 上驗證過的 eSIM 切換流程。

也感謝 litehtml、FreeType 和 GNU Unifont。原作者和授權聲明都已保留。本儲存庫不包含中興韌體、私人設定或原始參考壓縮檔。

## 版權和授權

各部分的版權歸各自作者所有：
- OpenUI：David Klasens 和 Jesther Silvestre；
- 本專案延續的分支：ZweiChen；
- 本儲存庫新增的內容：acloudpeng。

| 元件 | 授權 |
| --- | --- |
| OpenUI agent、控制台和通用工具 | [MIT](LICENSE)，保留 David Klasens 和 Jesther Silvestre 的聲明 |
| 單獨建置的螢幕程式和改編的 DRM 程式碼 | [GPL-3.0-or-later](screen/LICENSE) |
| 內嵌的 GNU Unifont OTF 字型 | [SIL Open Font License 1.1](screen/FONT-LICENSE) |
| lpac（單獨的 eSIM 程式） | [AGPL-3.0-only](licenses/lpac/)，其中 libeuicc 為 LGPL-2.1-only，cJSON 為 MIT |
| 相依套件和下載的工具 | 各自上游的授權 |

請見 [NOTICE](NOTICE)、[螢幕程式聲明](screen/NOTICE)和[保留的上游授權](licenses/)。根目錄的 MIT 授權不會改變 GPL 螢幕程式碼或第三方字型的授權。
散布 GPL 螢幕程式時，必須附上對應的原始碼和相關聲明；螢幕程式的原始碼和建置指令碼都在本儲存庫裡。
