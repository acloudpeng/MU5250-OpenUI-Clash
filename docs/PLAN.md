# 开发规划：OpenUI + WireGuard + Clash.Meta（FlClash 内核）

目标设备：ZTE U60 Pro（MU5250），固件 **CN B31**，全新未解锁。
目标：设备自托管的网页管理（OpenUI）+ 两种互斥的代理后端（内核 WireGuard / mihomo）。

## 上游与基线

| 来源 | 仓库 | 基线 commit | 用途 |
|---|---|---|---|
| 主干 | czwstc/MU5250-Wireguard | `5834b2e` | 本仓库直接从它开始（含完整 OpenUI + WireGuard + DevUI） |
| OpenUI | dklasens/MU5250-OpenUI | `fabaf8b`（v2.4） | WireGuard 分叉的导入基线；与本仓库**无共享 git 历史**，同步时按 diff 移植 |
| 原作 | jesther-ai/open-u60-pro | `fdd048a` | 按需移植 OpenUI 缺失的 agent 功能（P8） |
| 代理内核 | chen08209/Clash.Meta 分支 `FlClash` | `d0c8d443298ce26e292a63f0f1016313dda599ac` | 构建 `mihomo` 二进制，固定 commit |

remote 名：`upstream-wireguard`、`upstream-openui`、`upstream-original`。

## 已确认的设计决定

1. **不做三仓库平行合并**：WireGuard 分叉已是 OpenUI 的超集；原作功能按需移植。移动端 App 不在范围内。
2. **WireGuard 保留，与 Clash 互斥**：状态只有 `都关 / WG 开 / Clash 开`。
   - 打开一个时若另一个开着，界面确认后由 agent 在一个事务内切换：HOLD → 拆旧策略 → 建新策略 → RELEASE；失败回滚到旧后端。
   - 两个后端**各自保存设备选择名单**，各自的配置档互不影响。
3. **mihomo 构建**：`GOOS=linux GOARCH=arm64 CGO_ENABLED=0`，tags
   `with_gvisor,with_low_memory,no_tailscale,no_zerotier,no_easytier`。**禁止 `GOOS=android`**（会进入嵌入模式，禁用配置重载）。
   - FlClash 分支是给 App 嵌入用的：`hub/executor/executor.go` 里 `updateListeners` / `updateTun` 被注释掉（由 App 的 Go 胶水自己启动），
     单独运行时 **`tun:` 和 `mixed-port` 都不生效**。`scripts/mihomo-patches/0001-standalone-listeners-and-tun.patch` 恢复这两行，
     构建时自动打补丁。其余差异（订阅 provider 异步加载、hook、DNS 追踪）对独立运行无害。
4. **不用 mihomo 的 auto-route / auto-redirect**：固件是 fw3 + iptables-legacy（B27 数据，B31 待确认），auto-redirect 依赖 nftables。
   改用 WireGuard 分叉已验证的方案：`br-lan` 上按 MAC 打 fwmark → `ip rule` → 独立路由表 → `ou-clash0`（TUN，`auto-route: false`）。
   kill-switch、HOLD/RELEASE、revision/409、30 s 看门狗全部复用。
5. **mihomo 由 agent 作为子进程监管**，不新增 rc.local 行（遵守 SAFETY.md）。参考原作 Tailscale 模块：等 WAN、退避重启、WAN 重连事件、wakelock、`GOMEMLIMIT`。
6. **控制接口**走 `external-controller-unix`，不在局域网开放端口；网页只经 agent 的 `/api/clash/*` 访问。
   mihomo 的 unix socket **不校验 secret** 且 chmod 0666，所以安全性靠 socket 所在目录 0700（仅 root），不再设 secret。
7. **订阅 = URL 下载完整 Clash YAML**：
   - agent 调用设备自带 `/usr/bin/curl` 下载（不给 agent 引入 TLS 依赖）；UA 默认 `clash.meta`，30 s，上限 8 MiB。
   - 读取 `subscription-userinfo`（流量/到期）、`profile-update-interval`（自动更新）、`content-disposition`（默认名）。
   - base64 节点列表第一版直接报错。
   - 合成：保留 `proxies / proxy-groups / rules / *-providers / dns 上游 / sniffer`；
     强制覆盖 `tun`、端口（仅 127.0.0.1 mixed-port）、`allow-lan: false`、`dns.listen/enhanced-mode(fake-ip)/fake-ip-range`、
     `external-controller-unix`、删除 `external-ui / iptables / interface-name / routing-mark`、
     `find-process-mode: off`、`geodata-loader: memconservative`、`profile.store-selected: true`。
   - `mihomo -t` 校验通过才替换，失败保留上一份可用配置；`PUT /configs` 热重载。
   - 手动更新、按间隔自动更新、可选"通过代理更新"（curl 走 127.0.0.1 mixed-port）。
   - 最多 5 个订阅；订阅 URL 视为密钥，API 只返回打码值。
   - geo 数据随部署包推送（国内首启无法从 GitHub 下载）；`geox-url` 可配镜像。
8. **DNS / IPv6**：被选设备的 53 端口（UDP/TCP，任意目标）DNAT 到 `198.19.0.53`，该地址经标记路由进入 `ou-clash0`，
   由 TUN 的 `dns-hijack: [any:53, tcp://any:53]` 接管；mihomo 自身 DNS 只听 `127.0.0.1:1053`。
   INPUT 放行 `-i ou-clash0`（回包）。IPv6 先阻断（与 WG 一致）。未选设备与路由器自身不受影响，dnsmasq 不做持久修改。

## 阶段

| 阶段 | 内容 | 依赖设备 | 状态 |
|---|---|---|---|
| **P0** | 解锁（`zunlock.py --dry-run` 先行，需 backup-key 后缀）→ OpenUI `setup.sh` 装基础 + SSH → 只读探测脚本 → `docs/B31-PROBE.md` | 是（**等后缀**） | **完成（2026-10-10 实机）**：B31 解锁成功，suffix 有效；agent + SSH + 网页 + mihomo + geo 已部署，重启自启验证通过。过程见 `docs/OPERATIONS.md` |
| **P1** | mihomo 交叉编译（CI + 本地 Docker）、`prepare-mihomo.py`、sha256 固定；实机基准（内存/CPU/吞吐） | 基准需要 | 构建完成；**内核已在实机部署并校验 sha256**，吞吐基准待测 |
| **P2** | 从 `wireguard.rs` 抽出 `egress_policy`（设备、标记位、路由表参数化）；WG 行为不变；互斥切换事务与回滚 | 否 | 完成（互斥切换在 P3 实现） |
| **P3** | `clash.rs`：进程监管、订阅子模块、配置合成、Unix socket 控制客户端、策略应用、revision/409 | 否（mock） | 完成（netns 数据面测试通过）；**`/api/clash` 在实机返回 `core_installed/tun_supported/geodata` 正常**，待配订阅 |
| **P4** | 网页「代理」分组：概览 / 订阅 / 节点组 / 设备分流 / 连接 / 日志 / 高级设置；`api.ts`、`mock_agent.py`、契约检查 | 否 | 完成（mock + 浏览器走查）；**网页 8080 实机可访问** |
| **P5** | `deploy-components.py --mihomo`、`transaction.sh` 快照目标、geo 数据打包 | 否 | **完成（实机部署成功，事务快照已生成）** |
| **P6** | B31 实机验证：吞吐、DNS/IPv6/kill-switch 泄漏、防火墙重载/重启/WAN 重连恢复、72 h 内存浸泡、WG↔Clash 切换 | 是 | 进行中：**部署 + 重启持久性已验证**；吞吐/泄漏/浸泡待测 |
| **P7** | DevUI 小屏：后端状态、Clash 开关/模式/节点 | 否（预览） | 完成（预览测试 + aarch64 交叉编译，见 2026-10-10 阶段 4），待实机 |
| **P8** | 移植原作功能：端口转发、DMZ、SIM PIN/PUK、访客 Wi-Fi、飞行模式/数据开关、网络搜索；TLS 类（短信转发/测速）借 mihomo mixed-port | 否 | 可选 |

## P0 探测项（只读）

- 固件版本字符串、内核版本、`/proc/config.gz` 是否存在
- `/dev/net/tun`、`CONFIG_TUN`；`/sys/module/wireguard`
- `iptables -V`（legacy/nft）、`iptables -j TPROXY -h`、`-m socket`、`-m mac`；`nft list tables`
- `ip rule`（mwan3 占用的优先级/表号）、QCMAP 链位置
- IPA 硬件加速：打标记计数器 vs 实际流量
- `/usr/bin/curl -V`（HTTPS 支持、协议列表）
- `df /data /tmp /etc`、`free`、autosleep/wakelock 节点

## 风险

1. IPA 硬件加速可能让转发流量绕过 netfilter → MAC 打标记失效（P0 首要确认）。
2. B31 内核可能没有 TUN → 退到 TPROXY（需 `xt_TPROXY`/`xt_socket`）→ 再退到 REDIRECT（仅 TCP）。
3. QCMAP 把规则插回第 1 位、mwan3 改 `ip rule` → 看门狗修复；开机/重载期间有短暂泄漏窗口。
4. 4× A55 用户态代理吞吐有限，被代理设备失去硬件加速。
5. B31 解锁流程未经上游验证（上游只验证 HK B04）。~~**已解除：2026-10-10 实机解锁成功**~~。
6. mihomo 与屏幕程序为 GPL-3.0，分发需给出源码 commit。
7. 用上游 OpenUI 安装器更新会覆盖本仓库新增内容。
8. **suffix 轮换后无法恢复**（2026-10-10 实机发现）：`docs/DEPLOYMENT.md` 记载的
   "从 `strings /usr/sbin/zte_web` 提取新 suffix" 在 B31 上**不成立** —— 守护进程实际在
   `/usr/bin/zte_web`，且 suffix 既不是明文、也不是单字节 XOR（255 个密钥全试）。
   `zte_topsw_mc`（真正执行 `device_backup_proc` 的二进制）和 `libztemc.so`（含备份
   `openssl` 模板）里同样没有。**全固件 1524 个二进制扫描零命中**，且**不是**从 IMEI /
   `/proc/zte_unique_id` 可推导（49 种推导全部失败，用真实备份作 oracle 验证）。
   **固件更新若导致 suffix 轮换，可能永久失去解锁能力。**
   完整证据见 `docs/reference/suffix-recovery-b31.md`；更新前务必先按
   `docs/OPERATIONS.md` §6.2 抓取指纹。
9. ~~**Windows 主机执行 `deploy-components.py` 需包装**：管道 `selectors`、
   `os.killpg`、CRLF 双重转换、宿主机无 `sh -n`（见 `docs/OPERATIONS.md` §7）。~~
   **已解除（2026-10-10）**：部署工具在 Windows 上原生可用，`deploy-windows.cmd` 一键更新。
10. **rc.local 可能被宿主机引号问题截断**（2026-10-10 实际发生）：见 `docs/SAFETY.md`。
   恢复靠事务快照 `before/etc_rc.local`。

## 进展记录

### 2026-09-25 — P1 构建
- `scripts/prepare-mihomo.py`：固定 commit `d0c8d44` + Go 1.26.8，tags
  `with_gvisor,with_low_memory,no_tailscale,no_zerotier,no_easytier,no_fake_tcp`（`no_fake_tcp` 去掉会调用 iptables 的 hysteria faketcp）。
- 静态 aarch64 ELF，**34.7 MiB**，构建**可复现**，sha256 `fc1bdfeb…a874305` 写死在脚本中，不符即失败。
- qemu 下 `mihomo -v` 为 `linux arm64`（非 android 嵌入模式）；`tests/mihomo/smoke.yaml`（TUN `auto-route: false`、
  unix 控制接口、fake-ip、wireguard 出站节点）通过 `mihomo -t`。
- CI：`.github/workflows/mihomo.yml` 构建 + qemu 冒烟 + 上传 artifact。
- 待实机：RSS/CPU/吞吐基准（P1 剩余部分，需 P0 完成）。

### 2026-09-25 — P2 抽出出口策略
- `agent/src/egress.rs`：`Egress { label, prefix, mark, table, iface, dir, owner, masquerade }`，
  生成策略 / 清理 / HOLD / RELEASE / 健康检查脚本，`script()`、`claim()`、`owned()`。
- `wireguard.rs` 改用 `const EGRESS`（`OUWG`、`0x40000000`、表 `10420`、`masquerade: true`）。
- 行为不变的证据：重构前后用 `OPENUI_WG_TEST_POLICIES` 导出 all/selected/excluded/cleanup/hold/release/health
  七份脚本，逐字节一致；70 个单元测试通过。
- Clash 将用 `OUCL`、`0x20000000`、表 `10421`、`ou-clash0`、`masquerade: false`（mihomo 需看到真实 LAN 源地址）。
- 顺带修复上游 `screen_shortcut.rs` 在新版 clippy 下的 `useless_conversion`（CI 失败原因）。
- 探测脚本增加已占用 fwmark 列表，确认两个标记位与固件无冲突。

### 2026-09-25 — P3 agent 后端
- **FlClash 内核问题**：单独运行时 TUN 不起、mixed-port 不监听。原因见决定 3；加补丁后重新构建，
  版本串 `flclash-d0c8d44-openui`，sha256 更新为 `330642b1…a79f77`（两次构建一致）。
- `agent/src/clash.rs`：设置（revision/409）、最多 5 个订阅（下载→`mihomo -t`→原子替换，失败保留旧档，URL 打码）、
  子进程监管（pidfile、`GOMEMLIMIT=128MiB`、日志环形缓冲、5 s 看门狗、5 s→300 s 退避、到期自动更新订阅，失败 1 h 后重试）、
  代理模式 / 日志级别走 `PATCH /configs` 热更新，其余改动 `PUT /configs?force=true` 重载。
- 策略复用 `egress.rs`：`OUCL`、`0x20000000`、表 `10421`、不做 masquerade（保留 LAN 源地址）、`accept_input`。
- **WG↔Clash 互斥**：两边 `update` 都要求 `takeover: true` 才能在另一边开着时打开；锁顺序固定为先 WG 后 Clash；
  切换在 HOLD 内完成（停旧 → 建新 → 持久化），任何一步失败回滚到旧后端。两边看门狗发现另一方也开着时只报错不动作。
- API：`GET/PUT /api/clash`、`POST /api/clash/subscription/refresh`、`GET/PUT /api/clash/proxies`、`POST /api/clash/delay`、
  `GET/DELETE /api/clash/connections`、`GET /api/clash/logs`；`api.ts`、`types.ts`、`mock_agent.py` 同步，契约检查通过。
- 测试：新增 8 个单元测试；`live_controller`（`OPENUI_CLASH_TEST_SOCKET`）对真实内核通过；
  `tests/clash-netns.sh`：导出的 agent 策略 + qemu 下的真实内核，在 netns 中验证 all/selected/excluded 分流、
  UDP/TCP 任意 DNS 被接管、内核停止时断网断 DNS 且管理口可达、重启恢复、关闭后直连、HOLD/RELEASE。已加入 `mihomo.yml` CI。
- WG 七份导出脚本与重构前逐字节一致。

### 2026-09-25 — P4 网页「代理」
- 新导航项 **Proxy**（`web-app/src/features/proxy/`）：概览（开关、状态、代理模式 Rule/Global/Direct、TUN 栈 / DNS 模式 / 日志级别）、
  订阅（增删改、切换、立即更新、流量/到期条、URL 只显示主机）、节点（按组选择、单节点/整组测速）、设备分流（与 WG 相同的全部/选中模式，
  名单独立）、连接（3 s 刷新、筛选、全部关闭）、日志（跟随/暂停）。
- 互斥在两边界面都有确认框：WG 开着时打开 Clash、Clash 开着时打开 WG，都会先确认再带 `takeover: true`。
  WireGuard 状态新增 `clash_enabled`（只读文件，不取 Clash 锁）。
- 设备名单编辑沿用 WG 的"编辑时的 revision"，别处改过会得到 409 而不是静默覆盖。
- 走查：Playwright（容器）对 mock 跑完整流程——登录、添加订阅（确认 token 不回显）、WG→Clash 接管确认、模式切换、选节点、测速、
  设备保存、连接关闭、日志、手机宽度——无控制台错误。

### 2026-09-25 — P5 部署
- `scripts/prepare-geodata.py`：下载 MetaCubeX 的 `geoip.metadb`、`GeoSite.dat`（`--asn` 另加 `ASN.mmdb`），
  逐个对照上游发布的 `.sha256sum`，结果写入 `SHA256SUMS`。上游每天重建、无版本标签，所以不固定哈希，只保证与发布一致；
  `--base` 可换 HTTPS 镜像。
- `deploy-components.py`：`--mihomo`（aarch64 ELF + 固定 sha256 + 版本串，`--allow-unpinned-mihomo` 放宽哈希）、
  `--mihomo-geodata`（只接受白名单文件名，逐个核对 `SHA256SUMS`）。设备上：内核推到 `mihomo.staged`，`-v` 核对版本串后再替换；
  geo 放 `/data/share/mihomo/`；替换后停掉正在跑的内核，由 agent 看门狗重启（期间 kill-switch 挡住代理设备）。
  顺手修了 wg/屏幕程序安装命令缺 `set -e`、自检失败仍会替换的问题。
- `transaction.sh` 快照新增 `data/bin/mihomo`、`data/share/mihomo`；预检的空间估算包含 `/data/share/mihomo`。
- agent：每次准备目录时把 `/data/share/mihomo` 里的 geo 文件软链接进内核 home（覆盖内核以前自己下载的），状态新增 `geodata`；
  网页概览在缺 geo 数据时提示。
- 验证：`unshare -n`（无网络）下内核通过软链接加载 GEOIP/GEOSITE 成功，缺文件时尝试访问 GitHub 并失败——已加入 `mihomo.yml` CI。
  新增部署/快照单元测试（伪造 transport 检查推送路径、命令顺序、篡改/越界文件名被拒）。
- 文档：`docs/CLASH.md`。

### 2026-09-25 — 代码审查修复
独立审查 P2–P5 发现 10 个问题，全部确认并修复：
- 首次打开 Clash 失败：保护链（HOLD）建好后才 `claim()`，被自己的 `OUCL_` 链拦住。改为先 claim 再上保护。
- 修改当前订阅 URL 失败时，回滚用的是新 YAML，局域网一直被挡。改为先还原订阅文件和元数据再回滚策略。
- `--harden` 会把 mihomo 和 geo 文件当成加固程序推进 `/data/bin`。现在只传加固程序。
- 恢复（resume）失败会让保护链永远留着：失败时也释放保护、清理半成品策略，看门狗发现 Clash/WG 都关着但保护仍在时自动释放。
- WG 看门狗的"Clash 也开着"冲突错误解决后不清除。现在会清除。
- 订阅刷新失败时 profile 和元数据不一致。改为连同 profile、元数据一起还原。
- 添加订阅先下载再检查 revision/上限/重名。改为先做便宜的检查（`precheck`）。
- 状态接口每 5 秒轮询都会调用 iptables。健康结果缓存 10 秒，看门狗仍实时检查。
- `cleanup_policy` 用 `strip_suffix().unwrap()` 截脚本，改为参数控制是否删网卡。
- WG 持久化逻辑重复，改为复用 `persist()`。
新增测试：`precheck`、策略清理不删 TUN、`--harden` 只推加固程序。

### 2026-09-25 — 管理面板中英文切换
- 默认简体中文，侧边栏底部、手机顶栏、登录页右上角的 **EN / 中文** 按钮切换，选择保存在 `localStorage`（`u60.lang`），
  切换时整个界面重新挂载（轮询数据有缓存，不会闪空）。
- `web-app/src/i18n/`：`t('英文原文', {变量})` 以英文为键，译文按区域放在 `zh/<area>.ts`（约 870 条）；
  模块级常量用 `N_()` 标记、渲染时 `t()`；日期按界面语言格式化。
- Agent 返回的英文错误在前端用 `tm()` 翻译（`zh/agent.ts`，约 310 条，精确匹配 + `{占位符}` 模式，
  "; " 连接的复合错误逐段翻译）；内嵌 mihomo / iptables / 固件原始输出的部分保持原文。
- `tools/check-i18n.cjs`（纳入 `npm test`）：用 TypeScript AST 找出未经 `t()` 的英文界面文字，并检查缺失、冲突、占位符不一致、未使用的译文。
- 走查：Playwright 对 mock 在桌面和手机宽度遍历全部页面和标签页，无控制台错误；切换英文后刷新保持英文，再切回中文正常。
  手机底栏"调制解调器"换行，改为"蜂窝"。
- DevUI 设备屏幕（320×480）菜单当时仍为英文，见下一节。

### 2026-09-25 — DevUI 屏幕语言同步 + 界面优化
- 语言同步：DevUI 设置新增 `language`（`zh`/`en`，默认 `zh`，旧设置文件读入为中文）。
  新接口 `PUT /api/screen/language`：面板登录后和每次切换语言时发送；语言变化时 revision +1，相同语言不写盘。
  控制 socket 的 `C` 记录末尾加语言字段，屏幕程序据此切换；旧 agent 不带该字段时按中文处理，非法值整条丢弃。
  `PUT /api/screen/settings` 可不带 `language`（保留原值）。DevUI 网页显示"屏幕语言：跟随本管理面板"。
- 屏幕文字全部改为 `L(英文, 中文)`，含对话框和失败提示；删除 `ui/menu.html`，菜单头部改由 `render.h` 生成。
- 界面优化：
  - Unifont 是 16px 点阵字体，原来的 14/18/22px 缩放发糊，改为 16/32px（标题 24px）；渲染器对粗体做双线描边。
  - 菜单：顶部状态卡同时显示电量和 WireGuard 状态（彩色圆点）；功能行加右箭头。
  - 概览：标签左、数值右对齐，CPU/内存加用量条（≥60% 琥珀色，≥85% 红色），温度放电池卡右上角，运行超过一天显示天数。
  - WireGuard：状态圆点 + 颜色，上下行流量用两个小卡片；"关闭"按钮用红色系；配置/设备分流为"标签—数值—箭头"行。
  - 已保存配置：当前配置绿色边框 + "当前"徽标，不再显示成禁用按钮。
  - 设备分流：方形勾选框（选中为绿色 ✓）、在线状态圆点；翻页/保存按钮统一 48–56px 触控高度。
  - 对话框加暗色遮罩，卡片收紧。
- 验证：本机预览构建（`sh screen/build.sh --preview`）的点击/导航/转义测试全部通过，新增语言切换断言（中文默认、`en` 切换、
  旧格式 `C` 记录、非法语言拒绝），并逐页查看了中英文截图；zig 0.16 交叉编译 aarch64-musl 通过。
  Agent 新增测试 `language_follows_the_web_panel`。真机效果仍需设备解锁后确认。

### 2026-10-09 — 实机：整组测速报 `get delay: all proxies timeout`
- 现象：B31 实机「节点」页整组测速报 `Clash 内核：get delay: all proxies timeout`。mihomo 在组内每个节点都测速失败时返回这句，单个失败原因不会透出。
- **主要怀疑：B31 系统时钟快 8 小时。** 其他 B31 项目实测，固件把北京时间当作 UTC 写进系统 epoch（`TZ=UTC`），原厂 NITZ/NTP 还会写回
  （gaofeng21cn/u60-pro-enhanced `docs/INSTALL.md`、peter5842/U60-Pro-WebUI `agent/src/mihomo/config.rs`）。
  VMess 客户端时间戳取内核的 `ntp.Now`，服务端按自身系统时钟只允许 ±120 s（sing-vmess `service.go`）；SS2022、部分 REALITY 服务端同样校验时间。
  节点全是这类协议时，所有节点失败，整组测速就报这句。
  - 复现：本机起一个 VMess 节点（服务端按真实时钟校验）。客户端协议时钟快 8 小时（用假 NTP 模拟），报 `get delay: all proxies timeout`；
    客户端用新生成配置里的 NTP 段，返回 `{"vmess-node":202}`。
  - 修复（agent）：`clash_config.rs` 生成的配置固定带
    `ntp: {enable: true, server: 203.107.6.88, port: 123, interval: 5, write-to-system: false}`。
    `203.107.6.88` 即 ntp.aliyun.com，直接写地址，校时不依赖 DNS。只校正内核自己的协议/TLS 时钟，不改系统时间。订阅里的 `ntp` 照旧丢弃。
    间隔 5 分钟：开机时 WAN 未就绪导致首次校时失败，最多 5 分钟后重试（mihomo 失败后要等一个间隔）。
    `162.159.200.1`（Cloudflare，gaofeng 的选择）从开发机超时，故未采用。
  - 注意：这个内核版本的校时日志方向写反了。系统时钟快 8 小时时日志写「behind NTP time by 8h」，只看数值。
- **内核缺内置根证书（构建缺陷，B31 上大概率不是这次的原因）。** mihomo 源码里的 `component/ca/ca-certificates.crt` 是 0 字节占位文件，
  上游在发布 CI 里填好再编译；`prepare-mihomo.py` 直接编译，内核只能用系统证书。
  其他 B31 项目读取 `/etc/ssl/certs/ca-certificates.crt`，原厂 curl 校验通过，说明 B31 上 Go 能找到系统证书，旧内核照样能做 TLS 校验。
  - 复现：按原脚本重建的内核与原固定 sha256 `330642b1…a79f77` 一致，内置证书 0 张。
    用 `DISABLE_SYSTEM_CA=true` 模拟没有系统证书的固件：同一节点 HTTPS 测速报 `all proxies timeout`，HTTP 正常。
  - 修复：编译前写入 curl 提取的 Mozilla 根证书（`cacert-2026-09-25.pem`，121 张，固定 sha256），编译后检查已嵌入。
    版本串改为 `flclash-d0c8d44-openui-ca20260925`，sha256 `df75037f…6688985`（三次构建一致）。
    新测试 `tests/clash-delay.sh`（已加入 `mihomo.yml`，需联网）：只用内置证书时整组测速必须成功，去掉内置证书的对照组必须失败。
- 验证：agent 97 个测试、fmt、clippy、Python 21 个测试通过。生成配置经 `mihomo -t` 通过。
  qemu 下新内核的 `-v`、`smoke.yaml`、离线 geo、`clash-delay.sh` 均通过。
  netns 数据面测试本机缺 `dig` 未跑。实机待确认：`date -u` 是否快 8 小时、节点协议、部署后测速。
- 部署：agent（NTP）和内核（证书）都要重新部署。
- 补记（同日）：CI 里 `clash-delay.sh` 首次运行失败，是测试自身的启动竞态——mihomo 先开控制接口（`hub.ApplyConfig` 里
  `applyRoute` 在 `executor.ApplyConfig` 之前），qemu 下节点的监听端口还没开就开始测速。测试改为等节点端口进入 LISTEN、客户端分组加载后再测。
  agent 不受影响：它等 TUN 网卡出现，TUN 在监听端口之后创建。
- 新增只读诊断脚本 `scripts/device/diagnose-clash.sh`：时钟偏差、生成配置是否带 ntp、内核 DNS、节点 HTTP/HTTPS 测速、经内核访问国内外地址、
  连接来源、TUN 与 rp_filter、`ip rule` 与标记流量的路由结果、钩子位置与计数器。真机现象「需要代理的网站打不开、直连的能打开」用它区分
  「节点不通」和「流量没进 Clash」。

### 2026-10-10 — 阶段 1：内置一键诊断
- 背景：实机上「需要代理的网站打不开、直连的能打开」，分不清是节点不通还是设备流量没进 Clash。
- agent：`agent/src/clash_diag.rs`，`GET/POST /api/clash/diagnostics`。POST 在后台线程运行（HTTP 只有几个工作线程，不能占用 40 秒），GET 轮询结果。
  八项检查：时钟偏差（国内站点 HTTP `Date` 对比）与生成配置是否带 ntp、内核控制接口与版本、内核 DNS、主分组当前节点的 HTTPS/HTTP 测速与整组测速、
  经 mixed-port 访问国内/国外、设备分流（哪些设备走代理、内核里有没有它们的连接）、标记包的路由（`ip route get … iif br-lan mark`）、
  防火墙钩子是否在链首/保护链残留/kill-switch 拒绝计数/IPA 加速/rp_filter。结论按层给出：clash_off、core_down、not_routed、nodes、no_devices、ok。
- 网页：概览页「Clash 诊断」卡片（运行中进度、结论与提示、逐项证据、设备列表、路由详情）和「设备分流」范围标签；旧 agent 返回 404 时提示部署新版。
- 验证：agent 单元测试（HTTP 日期、设备选择、钩子顺序、计数、结论、主分组解析）；`live_diagnosis` 在容器里对真内核跑通（节点 HTTP 93 ms/HTTPS 296 ms，
  无策略时正确判为 not_routed）；网页 build/lint/test、i18n 0 问题、契约检查通过；mock 下中英文桌面/手机截图无控制台错误。
- 调研（其他 B31 项目）：本仓库的 fwmark→表 10421→TUN 方案源自 czwstc 的 WireGuard 分支，只在 netns 里测过、未在真机验证；
  B31 上验证过的做法有 mihomo 自带 auto-route（规则 9000–9099）+ FORWARD 首位放行，以及打标记 + 自建路由表并处理固件重定向链；
  有项目要求关闭 IPA 加速，担心转发流量绕过 netfilter。等用户的诊断输出再决定是否改数据面。

### 2026-10-10 — 阶段 2：统一「代理」页面
- agent：`agent/src/proxy.rs`，`GET/PUT /api/proxy`。GET 用无锁读取（`wireguard::get()` 会等 WG 锁，而 Clash 切换可能持锁两分钟），
  给出当前后端、冲突标记、两边的 revision、是否就绪和原因；PUT `{backend, expected_revision}` 调用两边已有的 update（`takeover: true`），
  锁顺序、HOLD、回滚和 409 都不变。两边都保存为开启时，关掉没被选中的那个（选「关闭」则两个都关）。
- 网页：侧边栏去掉 WireGuard（手机底栏 8 → 7 项）；「代理」页顶部「当前代理：关闭 / WireGuard / Clash」，未就绪的选项置灰并给出原因，
  切换和关闭前确认；下面「配置」切换 Clash / WireGuard 视图；WireGuard 拆成状态/已保存配置/设备分流子页并去掉启用开关；
  Clash 新增「高级」页（TUN 栈、DNS、日志级别从概览移来）。旧 agent 没有 `/api/proxy` 时提示部署新版。
- 验证：网页 build/lint/test、i18n、契约检查；mock 下浏览器走查 106 项（切换、接管确认、409、未就绪、冲突、中英文、桌面/手机）。

### 2026-10-10 — 阶段 3：设备上更新内核与 Geo 数据
- 用户决定：部署脚本继续装固定的 FlClash 构建；设备上可在线换成 MetaCubeX 官方版，也能上传自建内核。
- agent：`agent/src/clash_core.rs`，`GET/POST /api/clash/core`、`PUT /api/clash/core/upload`（在通用 1 MiB 读取之前流式写盘，先过登录校验，上限 64 MiB）。
  检查更新只问 api.github.com（缓存 10 分钟），摘要只取 GitHub 发布信息；下载可经 Clash、直连或镜像前缀；flate2 解压（上限 160 MiB）、
  aarch64 ELF 检查、`-v` 含版本号、用当前配置 `-t`；取部署事务锁后原子替换并硬链接保留 `mihomo.previous`；Clash 开着时重启内核，
  15 秒内不应答就自动换回。Geo 数据核对发布的校验和，替换后重启内核（mihomo 把 MMDB/GeoSite 缓存在内存里，热加载不会重读）。
  `clash.rs` 抽出 `restart_locked()`（准备 → HOLD → 策略 → 释放）供看门狗和更新共用。状态存 `core.json`（`config.json` 是 deny_unknown_fields，旧 agent 会拒绝新字段）。
- 网页：Clash「高级」页「内核与数据」卡片（版本与来源、检查/更新/回滚、下载方式与镜像、上传、Geo 更新、进度与错误）。
- 验证：agent 单元测试；容器内对真实 GitHub 跑通 v1.19.32 下载、摘要、解压；上传/回滚/部署锁/中断恢复路径；网页 13 个测试、mock 走查 51 项。
  **未验证：设备上替换后重启内核**（这台开发机跑不了 arm64）。

### 2026-10-10 — 阶段 4：DevUI 触屏代理功能
- 菜单最多 4 项，可选 概览/代理/WireGuard/Clash/已保存配置/设备分流，默认前四个；恰好等于旧默认的设置迁移一次（`menu-v2` 标记文件）。
  已保存配置和设备分流始终能从 WireGuard 页进入。
- 页面：代理（关闭/WireGuard/Clash + 诊断）、Clash（状态、开关、规则/全局/直连）、节点（分组 → 节点，每页 4 个，点选、整组测速）、
  订阅（设为当前/立即更新）、内核与数据（检查/更新/回滚/更新 GEO）、诊断（结论 + 8 项）。WireGuard 开关改走代理切换，修复 Clash 开着时切换失败。
- 协议：新增 `V B K U M X/x E` 记录、`groups`/`nodes` 查询（按序号 + 代数，从缓存回答，不阻塞）、`backend/cmode/csub/crefresh/select/delay/core/diag` 动词；
  `J` 末尾加 `fresh`；状态回复 40 KiB 以内；`C` 只带旧屏幕认识的页面，旧屏幕与新 agent、新屏幕与旧 agent 都能工作。
  状态刷新拆成三个线程，WG 锁被占用时屏幕不再变灰。
- 验证：agent 测试、`screen/build.sh --preview` 新页面中英文点按测试（触控区 ≥48×48、翻页、对话框、代数过期、标志转换、最大页面 5220 字节、状态 23 KB）；
  zig 0.14.1 交叉编译 aarch64 通过。**未在真机运行**：部署屏幕后需跑 `verify-screen.py` 并手动走一遍。

### 2026-10-10 — agent HTTP 服务：tiny_http 资源上限
- 问题（原有，审查时发现）：tiny_http 0.12 丢弃未读完的请求体时按剩余 `Content-Length` 一次分配内存，局域网内任何客户端
  （无需登录）发一个声称几 TB 的请求就能让 agent 崩溃；请求行/请求头长度和数量也没有上限；连接没有读写超时，停发数据的客户端会一直占着线程。
- 修复：`vendor/tiny_http`（0.12.0 原样 + 补丁，根 `Cargo.toml` 用 `[patch.crates-io]` 引入并排除出 workspace；说明见 `OPENUI-PATCH.md`）：
  未读请求体按 8 KiB 分块丢弃、最多 1 MiB，超出则关闭连接；请求行和请求头单行 8 KiB、最多 100 个请求头；每个连接 60 秒读写超时
  （空闲的长连接一分钟后关闭）。随之删掉 `server.rs` 里给上传连接单独设超时的绕路代码。`Cargo.lock` 只有 tiny_http 一项改为本地路径。
- 验证：agent 新增回归测试（真实 socket：声称 999999999999999 字节的请求得到 413 而进程不崩；超长请求头行被断开、200 个请求头得到 400、正常请求不受影响）；
  139 个测试、clippy、fmt 通过，`cargo check --target aarch64-unknown-linux-musl` 通过。

### 2026-10-10 — 端到端测试（模拟路由器）与修复
- 方法：特权容器模拟 U60（独立网络命名空间，主机网络不变）：真实 agent（x86_64 构建）+ 真实 mihomo + iptables-legacy/策略路由/TUN，
  `client` 命名空间作局域网设备，`inet` 命名空间作互联网（SS 节点、国内/国外站点、DNS、NTP、WireGuard 对端、伪 GitHub 发布，测试 CA 签发 HTTPS），
  丢弃从 WAN 直连国外站点的流量模拟「墙」。11 个场景全部通过：基线（国外不通）、经 API 开 Clash 后国外走节点/国内直连/DNS fake-ip、诊断三种结论、
  测速、kill-switch、WireGuard 接管与回切、409、DevUI 套接字协议、内核更新（到 `-v` 前全部正确，x86 上按预期停在「无法在本设备运行」）、
  tiny_http 上限、真实 agent 的网页走查（中英文 × 桌面/手机 70 项）。
- 额外探测发现并已修复：
  - 未登录客户端声明请求体却不发送（或慢速滴发），丢弃请求体会阻塞 agent 的 4 个工作线程，API 停摆一分钟以上。改为丢弃时把 socket 临时设为非阻塞，
    只读已到达的数据，读不完就放弃该连接；连接自己的线程随后做延迟关闭（最多 2 秒读掉客户端还在发的数据再关闭），避免 RST 让客户端丢失响应。
    延迟关闭不提前关闭写方向：请求体提前结束（客户端只发一部分就半关闭）时，tiny_http 读到 EOF 就丢弃请求体读取器，此时处理程序还没写响应，
    先发 FIN 会让随后的 400 丢失（重跑端到端时发现，已加回归测试）。同理延迟关闭只缩短读超时：之前连写超时也改成 200 ms，
    这种情况下较大的响应遇到读得慢的客户端会被截断（复查时发现，已加回归测试）。
    处理请求的线程不再等待客户端。顺带修复上游 bug：`HTTP/2.0` 请求行会让连接线程死锁、泄漏线程和描述符，改为经请求自身的写出端回 505。
  - Clash 关闭时下载/更新订阅不做 `mihomo -t`，坏配置会被保存。现在每次下载都先按当前设置合成并检查，不通过则保留原配置并给出内核原因。
  - 配置被拒时报错只显示「test failed」，现在取内核的 `level=error` 行。
  - 过渡保护（HOLD）建到一半失败时 Clash 和 WireGuard 都会撤掉自己建的那一半（之前已存在的保护保留）。
  - DevUI 网页文案「打开英文触摸屏菜单」改为「打开触摸屏菜单」。
  - 小屏 `status` 里的代理记录（B/K/U/X）要等内核记录 M 一起存，而首次读取内核要算一遍文件哈希（可能几秒），刚启动时代理页空着。
    现在先存 B/K/U/X，M 算完后单独存。
- 模拟不了的：IPA 硬件加速、固件 fw3/QCMAP 规则与重载、真实运营商 DNS 与墙、B31 时钟快 8 小时、arm64 内核替换后的启动与回滚、屏幕程序本身。

### 2026-10-10 — 发布 v2.5
- 版本号统一为 2.5.0（agent、网页、安装器及各自的锁文件；`scripts/check-release.py` 要求它们与标签一致）。
- 发布工作流要求 `docs/images/installer-<标签>/` 下有 5 张安装器线框图：`installer/wireframes/capture.mjs` 改为按安装器版本命名输出目录和图上标签，
  重新生成了 v2.5 的图，删掉 v2.4 的（v2.4 标签里仍有）。
- 发布包里除了工作流产出的 `zte-agent`、`dashboard-dist.tar.gz` 和桌面安装器，另附 CI（mihomo core 工作流）编译的固定内核 `mihomo`，
  以及本机用 Zig 0.14.1 从该标签编译的 `openui-screen`（两次编译 sha256 相同：`77640772…ed82`）。GEO 数据不附带，用 `prepare-geodata.py` 准备。
- 桌面安装器从上游 `dklasens/MU5250-OpenUI` 下载包，不能用来更新本分支（发布说明已注明）。
  后续可做：发布工作流直接编译屏幕程序并附带内核；安装器改为指向本仓库，或不再随本分支发布。

### 2026-10-10 — eSIM 管理（可插拔 eUICC，对标 EasyEUICC）
- 需求：「新增 eSIM 管理，看看能不能内置 EasyEUICC」。结论：EasyEUICC 是 Android 应用，靠 Android 电话框架访问 SIM，装不到设备上；
  它的核心 lpac 能在 Linux 上跑。做法是内置 lpac，由 agent 转发它的卡指令和 SM-DP+ 请求，网页「蜂窝 → eSIM」和小屏 eSIM 页提供 EasyEUICC 的功能。
  只支持卡槽里的可插拔 eUICC（9eSIM、5ber、eSTK.me 等）；普通 SIM 会识别出来但不能下载；个别机型的 ZTE 内置 eSIM 不支持。
- 设备依据：社区项目 SadykovIV/ZTE_U60Pro 在 B31 上用 9eSIM V0 实测过（QMI over QRTR；切换后低功耗 → SIM 断电上电 → 联网），
  faying 的 manager 在 B27 上用 lpac 的 qmi_qrtr。SadykovIV 的 QMI/ES10 代码许可证不明确，只参考事实没有照抄，协议按 libqmi 的服务定义自己实现。
- lpac：固定 v2.3.0（上游新主线的驱动全部 dlopen，静态程序用不了），只编 stdio 的 APDU/HTTP 驱动，`scripts/prepare-lpac.py` 用 Zig 0.14.1 直接编译，
  源码包、补丁和输出 sha256 都固定（可复现）。9 个补丁：7 个上游修复的回移（v2.3.0 的 stdio APDU 驱动把成功当失败，根本用不了）；
  2 个自己的：① 带 `_cleanup_` 的变量没初始化，`chip info` 读 EID 失败（插普通 SIM）时释放野指针崩溃，逻辑通道也没关，上游主线同样有这个问题；
  ② stdio 驱动逐字符 strcat 读行，读 400 KB 的行要 6 秒（设备上更慢），下载配置包会卡，改用 getline 后 2 MB 只要 0.05 秒。
  CI 新增 `lpac` 工作流（固定 Zig、核对 sha256、qemu 下跑 `tests/lpac_stdio.py`），发布工作流自动附带 `lpac`。
- agent：`qmi.rs`（QRTR 名字服务查找 + QMI UIM：开逻辑通道、发 APDU、关通道、卡槽/卡状态、SIM 上下电；DMS 工作模式），
  `lpac.rs`（运行 lpac，转发 APDU 和 HTTP，超时杀进程，残留通道收尾），`esim.rs`（读卡、切换、停用、改名、删除、下载、通知、单任务、接口）。
  SM-DP+ 走设备 curl，TLS 信任 GSMA 的三个正式 CI 根（RSP2 Root CI1、OISTE GSMA CI G1、Entrust eSIM CA，同 EasyEUICC）加系统根证书；
  Clash 运行时默认经 Clash。下载/删除后自动发通知（同 EasyEUICC 默认），切换的通知留在卡上手动处理。
  切换：`lpac profile enable <iccid> 0` → 回读卡确认 → 低功耗 → SIM 断电上电 → 联网 → 等基带读到新 ICCID、USIM 就绪 → 等固件
  `get_sim_info` 报新 ICCID；固件没跟上就提示重启（用户要求：只提示，不自动重启），不重启任何固件守护进程。
  基带不支持「读卡槽状态」时以固件为准。调试版 agent 可用 `OPENUI_QMI_SOCKET_DIR` 走 Unix 套接字，供端到端测试模拟基带。
- 网页：「蜂窝」新增 eSIM 标签页（卡片信息、配置文件列表、切换/停用/改名/删除（输入名称确认）、激活码/二维码图片（jsQR 按需加载）/手填下载、
  确认码、IMEI、连接方式、通知）。小屏：新增 eSIM 菜单项和页面（列表、切换、重新读取、需要时重启）；菜单设置新增 `W` 记录（含 eSIM 页），
  旧屏幕继续读不含 eSIM 的 `V`。部署脚本新增 `--lpac`（固定哈希、暂存自检、纳入回滚快照）。
- 端到端：新增 S12（容器里：模拟基带 QMI UIM/DMS + 模拟 eUICC（ES10a/b/c、61xx 分段、下载会话、通知）+ HTTPS 模拟 SM-DP+ + 假 ubus，
  真实 agent 和 lpac，74 项）和 S13（主机上用浏览器走查 eSIM 页，中文桌面 × 英文手机）。发现并已修复：
  - `scan()` 在持有 `REBOOT_NEEDED` 锁时又加同一把锁：切换后固件没跟上、之后再读卡时任务线程死锁，随后所有 `GET /api/esim` 卡住，
    工作线程被占满后整个 API 无响应。改为只加一次锁，判断逻辑抽成纯函数并加测试。
  - 激活码第 5 段为 1（要求确认码）却没填确认码时，agent 照样开任务，lpac 在 PrepareDownload 才失败；现在直接 400「这个激活码需要填写确认码」。
  - 下载在写卡阶段失败（如 ICCID 已在卡上）时，卡上生成的错误安装结果没有发给 SM-DP+；按 SGP.22 现在失败时也发送，SM-DP+ 才知道这次下载失败。
  - 网页：删除完成的提示用删除后的列表找名字，只能显示 ICCID；现在记住页面见过的名字。
- 用户暂时没有 eUICC 卡：真机先用普通 SIM 验证「读取卡片」报「普通 SIM 卡」（即 QRTR/UIM 通路可用），有卡后再验证切换和下载。

### 2026-10-10 — 发布 v2.6
- eSIM 批次单独提交（`7217065`），之后版本号统一为 2.6.0，重新生成 v2.6 安装器线框图并删掉 v2.5 的（v2.5 标签里仍有）。发布说明 `docs/releases/v2.6.md`。
- 发布前在模拟路由器上跑了完整端到端：S1–S13、X1–X4 全部通过（S11 70/70、S12 74/74、S13 29/29）。
- 屏幕程序原来只在同一目录下编译才可复现：`panel.c` 保留了断言，`__FILE__` 带着检出目录的绝对路径（v2.5 的二进制里有 `/root/MU5250-OpenUI-Clash/screen/panel.c`）。
  `screen/devui/build.py` 加了 `-ffile-prefix-map`，现在在三个不同目录编出的 sha256 相同：`f104ee5e…b3f9`。
- 发布包新增发布工作流编译的 `lpac`（应等于 `prepare-lpac.py` 里固定的 `66641fd6…2c98`）；`mihomo` 与 v2.5 相同（`df75037f…8985`）；
  `openui-screen` 本机用 Zig 0.14.1 从该标签编译。
- SCREEN.md 的菜单功能列表补上 eSIM。真机还没验证的项目见发布说明。

### 2026-10-10 — Windows 一键更新（deploy-windows.cmd）
- 需求：「可以做一个 Windows 的一键脚本吗」。用户在连着设备的 Windows 电脑上部署，之前要手敲 POSIX 命令，
  `deploy-components.py` 在 Windows 上也跑不起来（PLAN 风险第 9 条）。
- 部署工具改成 Windows 上原生可用：
  - `run()` 在 Windows 上改用线程收发管道（`select()` 不支持管道），超时用 `process.kill()`，新进程组挡住控制台的 Ctrl+C；
  - 输出里的 `\r+\n` 统一成 `\n`；adb 临时文件先关闭再交给 adb；
  - 本机没有 `sh` 时，启动脚本的 `sh -n` 检查放到设备上做（通过 stdin，只读）；
  - 推给设备的 `scripts/device/*.sh` 去掉 CRLF；新增 `.gitattributes`（`*.sh` 固定 LF，`*.cmd` 固定 CRLF）。
- 顺带发现并修复：Windows 用户目录带空格时（如 `C:\Users\John Doe`），ssh 会把 `UserKnownHostsFile=` 的值按空格拆开，
  主机指纹记错文件、连接失败。现在加了引号（本机实测：不加引号时写到了路径的前半段）。
  `verify-screen.py` 原来用默认 known_hosts，在没手动连过设备的电脑上会因主机指纹未知失败，现在和部署工具共用 `known_hosts.d/zte`。
- 新选项：
  - `deploy-components.py --keep-credentials`：从设备的启动脚本读出现有密码和 PIN，不用再输入；
  - `deploy-components.py --skip-unchanged`：设备上 sha256 相同的组件不再部署（未变的 agent 不重启，未变的屏幕程序不用重新检查）；
  - `verify-screen.py --if-needed`：已通过检查的屏幕程序直接跳过。
  - 试运行阶段就检查触摸屏是否正在显示 OpenUI，不再等到事务中途失败；SSH 连不上时报 ssh 自己的错误（如 Permission denied），
    原来只有「Management operation failed」。
- `deploy-windows.cmd` → `scripts/windows/deploy.ps1`（兼容 Windows PowerShell 5.1，中英文提示按系统语言）：
  1. 找 Python 3.10+ 和 OpenSSH 客户端，缺 Python 时可用 winget 安装；
  2. 部署与检出版本对应的发布（2.6.0 → v2.6）：`scripts/download-release.py` 从发布页直接下载，走系统代理设置，按 sha256sums.txt 校验，不需要 gh；
     也可以用 `-AssetsDir` 指定已下载好的文件夹；
  3. 准备 GEO 数据；
  4. 试运行 → 确认 → 部署（保留密码和 PIN、跳过未变组件）；
  5. 屏幕程序变了才跑触摸屏检查。
  设备地址和私钥在试运行通过后记到 `build/windows-deploy.json`。有更新的发布时提示先 `git pull`。
  所有设备操作都交给 Python：PowerShell 5.1 给原生命令传带双引号的参数会出错。
- 测试：
  - 单元测试 `tests/test_deploy_portability.py`（11 项）：两种 `run()` 的输入、输出上限、超时、CR；替身 ssh 推送二进制；发布文件下载校验；
    真实 ssh 客户端报连接被拒；保留凭据、跳过未变组件、触摸屏占用、引号路径、启动器编码。
  - CI 新增 Windows 作业：在 Windows 上跑这些测试；在 Windows PowerShell 5.1 下解析脚本、检查帮助，并做一次冒烟运行。
  - 端到端新增 S14（`/root/MU5250-e2e/deploy/`）：模拟一台装着 v2.5 的 U60（SSH 2222、ubus/uci 替身，
    aarch64 程序经主机 qemu binfmt 运行）。用 pwsh 7 跑 deploy.ps1 升级到 v2.6，覆盖：
    - 升级、再次运行只换网页、已检查的屏幕程序跳过检查；
    - 触摸屏占用、私钥错误（且不会记住）、设备不可达、发布文件被改、取消，每种情况设备都不变；
    - CRLF 检出部署后设备上的脚本是 LF。
  - Windows 本身（PowerShell 5.1、winget、ssh.exe）的完整流程只能在用户电脑上验证。

### 2026-10-10 — 仓库公开
- 仓库设为公开，改名为 `U60Pro-OpenUI`（GitHub 会把旧地址跳转到新地址）。`deploy.ps1` 的默认仓库已同步。
- README 重写为公开首页：控制台、触摸屏、eSIM、安装与更新、文档、致谢和许可。截图仍在 `docs/images/`，首页不再引用。
- 发布页只保留 v2.6，说明按新首页改写；v2.4、v2.5 的发布已删除，标签保留。
- 一键更新改为直接从公开的发布页下载，不再需要 GitHub CLI 和登录。

### 2026-10-10 — 1.0
- 版本号改为 1.0.0（agent、网页、安装器和各自的锁文件），作为第一个公开版本发布 v1.0。原来的 v2.4–v2.6 发布和标签已删除。
- 提交历史统一了作者信息，所有提交号都变了；本文和 `scripts/device/diagnose-clash.sh` 里引用的提交号已同步更新。
- README 分成三个文件，顶部互相链接：
  - 简体中文（`README.md`，首页默认）；
  - 繁體中文（`README.zh-TW.md`）；
  - 英文（`README.en.md`）。
- 操作手册改名为 `docs/OPERATIONS.md`，其他文档里的链接已更新。
- 重新生成 v1.0 的安装器线框图。发布说明 `docs/releases/v1.0.md` 用三种语言，旧版本的发布说明文件已删除。
- 安装器测试里表示「版本不匹配」的样例标签从 v1.0 改为 v0.9。

### 2026-10-11 — 打开 DevUI 的方式：双指按住、开机打开、闲置熄屏
- 用户要求：不再依赖电源键（容易按坏），改成在原厂界面上长按进入，并可以开机直接进入。先调研了同类项目
  （mudi7、faying、33333s 等）：mudi7 实际是长按电源键；faying 的入口是长按屏幕角落 3 秒。用户选择：
  双指同时按住 2 秒；不另加锁；开机自动进入作为设置项；闲置后只熄屏、留在 DevUI。
- 双指按住（`agent/src/screen_touch.rs`，默认开启）：
  - 和连按快捷方式一样，不独占读取 `sitronix` 触摸屏（不用 EVIOCGRAB），原厂界面照常收到触摸；
  - 恰好两根手指、2 秒内移动不超过轴长的 1/12 才算；一根手指（原厂长按）、第三根手指、提前抬起、滑动都不算；
  - 打开时或丢事件（SYN_DROPPED）后，用 EVIOCGMTSLOTS 读回哪些手指按着，这些手指全部抬起后才重新计数；
  - 只在屏幕亮着时生效；触发后冷却 5 秒，手指抬起后才能再次触发；DevUI 运行中或开关关闭时照样读走事件，不会回放。
  - 原厂锁屏挡不住它（锁屏状态在外部读不到），连按快捷方式也一样；文档和 README 都写明了。
- 开机打开（`open_at_boot`，默认关闭）：每次开机一次。等 `/tmp/zte_boot_done`、`zte_topsw_devui` 运行、
  `get_sync_info` 同步完成，再等 5 秒后打开；关机充电（`mode_power_off`）不打开；agent 重启不重复
  （`/tmp/openui-devui-boot`）；5 分钟内固件没启动完就放弃，这次开机保持原厂界面。
- 闲置后的操作（`idle_sleep`，默认熄屏留在 DevUI）：
  - 设置经 `W` 记录的第 7 个字段传给屏幕程序；旧屏幕程序只读 6 个字段，仍然返回原厂界面；
  - `--supervise N`（恢复检查用）到时间一定退出；断开 agent 15 秒、子进程异常仍然恢复原厂界面；
  - 熄屏时保留当前页面和未保存的选择。
- 网页：DevUI 页的「电源键快捷方式」卡片改成「打开 DevUI 的方式」，三个开关；「闲置时间」和新的「闲置后」下拉框。
- 检查脚本：`verify-screen.py` 按保存的闲置设置测试（熄屏时确认 DevUI 还在、背光为 0，再关闭）；
  `verify-devui-settings.py` 依次测两种闲置操作。
- 需要在真机上确认：双指按住与原厂界面并存；开机打开；DevUI 熄屏停留较长时间（例如 30 分钟以上）设备不会自己重启。

### 2026-10-11 — 1.1
- 版本号改为 1.1.0（agent、网页、安装器和各自的锁文件），发布 v1.1，内容是上面「打开 DevUI 的方式」这批改动。
- 重新生成 v1.1 的安装器线框图，删除 v1.0 的。发布说明 `docs/releases/v1.1.md` 用三种语言。
- 安装器测试和线框图里的样例发布改为 v1.1；README 和 `deploy.ps1` 帮助里的版本示例改为 1.1.0 → v1.1。

### 2026-10-11 — 工单 #1：订阅里指向自己 DNS 监听的服务器；B31 时钟
- 用户在真机上（v1.0）遇到：订阅自带完整的 `dns:`，`listen: 127.0.0.1:7874`，
  `proxy-server-nameserver: [udp://127.0.0.1:7874]`。agent 把监听改成 `127.0.0.1:1053`，
  但指向旧端口的条目原样留下，节点域名全部解析失败（96 个节点 0 个可用）。
- 用户建议把这些条目改指向 1053。端到端实测（节点用域名、订阅不过滤节点域名）：
  - 原样：0/3；
  - 改指向 1053：0/3，节点域名经内核自己的监听拿到 fake-ip（198.18.0.5），连接失败；
  - 去掉：3/3，节点域名走主解析器。
  用户那边改指向 1053 能用，应该是订阅的 `fake-ip-filter` 覆盖了节点域名；去掉这些条目对两种订阅都有效。
- 修复（`clash_config.rs`）：所有 DNS 服务器列表和两种 policy 里，主机是本机回环（或未指定地址）、
  端口是订阅原来的监听端口或 1053 的条目一律去掉，列表空了就删掉这个键，回落到主解析器。
  `respect-rules` 或 `proxy-server-nameserver-policy` 要求 `proxy-server-nameserver` 非空时，
  用 `default-nameserver` 的服务器补上。
- 诊断的时钟检查：系统时钟快 8 小时（±120 秒）且配置里有 NTP 时判为通过，并标明是北京时间
  （网页和小屏都有说明）。节点失败时，时钟提示照常出现，以防内核 NTP 还没校时成功。用户真机实测偏差 +28 796 秒。
- 工单里另外两点的结论：
  - 「running: false 而 interface_up: true」「两个内核进程，父进程不同」：端到端里「选中的设备、MAC 列表为空」
    运行 60 秒，核心没有重启。最可能是 agent 重启的窗口：旧核心被 init 接管、继续撑着 TUN，
    新 agent 先用内核的 `-t` 检查配置，再替换旧核心（X3 覆盖）。部署时会出现，属预期。
  - 时钟见上。
- 端到端新增 S15（`tests/s15_dnsref.py`）：
  - 订阅 `setup/sub-dnsref.yaml` 的节点用域名 `hk01.node.example`；
  - 假服务器加了 `date_behind_8h` 开关，模拟 B31 的时钟。

### 2026-10-11 — 1.1.1
- 用户定的版本号：1.1.1（补丁版本），内容是工单 #1 的修复和时钟检查。
- 版本号改为 1.1.1（agent、网页、安装器和各自的锁文件）。发布说明 `docs/releases/v1.1.1.md` 用三种语言。
- 发布工作流原来按完整标签名找线框图目录（`installer-v1.1.1`），而线框图按主.次版本命名（`installer-v1.1`）。
  现在按主.次版本查找：补丁版本沿用所属次版本的线框图，安装器界面不变就不用重新截图。

### 2026-10-16 — Wi-Fi 中继（上游 Wi-Fi 作为上网通道）
- 用户要求「新增 wifi 中继功能」。选择：
  - 无线上网（WISP），不是同网段桥接：设备连上已有的 Wi-Fi，自己的热点、网段照旧；
  - 全部流量走上游 Wi-Fi（局域网设备和设备本身），上游掉线或没网时自动回到蜂窝网络，恢复后切回；
  - 网页和触摸屏都做，小屏只看状态和开关。
- 同类项目都没做过。faying 项目 2026-09-22 的真机实测是依据：在热点接口上扫描不影响热点；两个热点开着时能再加一个
  `managed` 接口；每次固件 Wi-Fi 重载都会重建 wiphy（phy 编号会变，临时接口随之消失）。
- 实现（`agent/src/wifi_uplink.rs`，设计见 `docs/WIFI-UPLINK.md`）：
  - 全部是运行时状态，不写 UCI、init.d、原厂防火墙链和主路由，不带 agent 重启后什么都不留。
  - 在热点所在的 phy 上加 `ou-sta0`，由 agent 运行设备上的 `wpa_supplicant` 和 busybox `udhcpc`。
    配置里的名称、WPA3 密码写成十六进制，WPA2 写成推导出的 64 位密钥，用户输入从不进引号。
  - 路由表 10431，规则 10429（来自中继地址）、10430（主表去掉默认路由）、10431（全部）。
    排在两个已有出口后端的规则（10420、10421）之后，它们管的设备不受影响，它们自己的连接走中继。
  - 防火墙链 `OUUP_*`：上游网络进不来（只收回包和 DHCP）；联网时局域网 DNS 和本机经中继发出的 DNS
    改发给上游网络的解析器；局域网 IPv6 转发暂停。
  - 联网检查：从中继地址 TCP 连 223.5.5.5:443、1.12.12.12:443、1.1.1.1:443。连续 2 次通过切到 Wi-Fi，
    连续 3 次失败切回蜂窝；断开关联立即切回。
  - 每 3 秒（保存后立即）检查一次：补回被固件防火墙重载冲掉的挂钩、缺失的规则、退出的进程；
    接口随固件 Wi-Fi 重载消失时整套重建。启动时先清掉上一个 agent 留下的联网规则，自己检查通过后再切过去。
- 接口：`GET/PUT /api/wifi/uplink`（带版本号，过期返回 409）、`POST /api/wifi/uplink/scan`。
  B31 时钟快 8 小时，所以另给 `online_for`（设备上算好的秒数）。
- 网页：「网络 → Wi-Fi 中继」：状态、开关（第一次打开前说明影响）、已保存网络（排序、编辑、删除）、扫描与加入、隐藏网络。
- 小屏：新页面「Wi-Fi 中继」（状态 + 开关），菜单可选，概览页有一行入口。新记录 `R`（状态）和 `Y`（带第 8 位页面的设置），
  新操作 `uplink <rev> on|off`；旧屏幕忽略 `R`，继续读 `W`。
- 端到端时发现并修复：联网检查原来用 53 端口，联网后本机经中继发出的 DNS 会被改发给上游路由器自己的解析器，
  上游没网时检查也能通过、永远不切回蜂窝。改成 443 端口。
- 端到端新增 S16（`tests/s16_uplink.py`、`uplink/`）：假 `iw`、假 `wpa_supplicant`（用 hashlib 独立核对 PSK），
  真 busybox `udhcpc`、iptables 和策略路由，`wifiup` 命名空间模拟上游路由器（DHCP、DNS、NAT）。
- 需要真机确认：设备上有没有可用的 `wpa_supplicant`（nl80211）和 `udhcpc`；热点旁加客户端接口能否正常关联；
  热点是否跟着换信道、已连设备断开多久；固件自己的 Wi-Fi 管理能否容忍这个额外接口。

### 2026-10-16 — Wi-Fi 中继：按 IP 的 HTTP 联网检查、网页认证模式
- 用户问「能输入密码吗？网页怎么登录认证」「测试连通性可以用 ip 吗」，随后要求两件一起做，做完跑端到端。
- 联网检查改成按 IP 发 HTTP 请求，不需要 DNS：`http://223.5.5.5/resolve?name=www.taobao.com&type=1`（JSON 里 `"Status":0`），
  不通再试 `http://119.29.29.29/d?dn=www.taobao.com`（地址列表）、`http://223.6.6.6/resolve?…`。
  回答对得上就是能上网；被跳转或返回别的页面就是需要网页认证，`Location`（或页面里的跳转地址）就是登录页；都没回应就是没网。
  `1.1.1.1` 的 80 端口在国内常不通，没有用。
- 新状态 `needs_login`（需要网页认证）、`logging_in`（认证模式）。认证模式 `POST /api/wifi/uplink/login {enabled}`：
  10 分钟内先把流量切到这个 Wi-Fi（规则、DNS 改发与联网时相同），手机或电脑打开登录页登录；检查每 3 秒一次，
  通过后立即转为正常联网并结束认证模式。到时、手动结束或关闭中继也会结束。上游看到的是路由器的中继地址，登录一次全部设备可用。
- 网页：需要认证时显示「开始网页认证（10 分钟）」，认证中显示倒计时、「打开登录页」链接和「结束认证模式」。
- 小屏：状态字母 `p`（需要网页认证）、`g`（认证模式），原因码 `portal`、`nologin`；需要认证或没网时「开始认证」和「关闭中继」并排，
  操作 `uplink login`（不用确认）。
- 已知限制：开着代理时，登录页可能被分到节点（节点在登录前不通），提示先关代理；只劫持 DNS 的认证页显示为没网，仍可开认证模式。
- 端到端 S16 新增：模拟上游路由器的认证页（`uplink/portal.py`、`portal.sh`），按 IP 的 HTTP 解析接口（inet 上 223.5.5.5、119.29.29.29、223.6.6.6 的 80 端口）；
  覆盖需要认证、认证模式开始和结束、局域网电脑经中继登录、登录后联网。

### 2026-10-16 — 1.1.2
- 用户定的版本号：1.1.2（补丁版本），内容是 Wi-Fi 中继（含按 IP 的联网检查和网页认证模式）。
- 版本号改为 1.1.2（agent、网页、安装器和各自的锁文件）。发布说明 `docs/releases/v1.1.2.md` 用三种语言。补丁版本沿用 v1.1 的线框图。

### 2026-10-16 — 合并用户的界面分支 feat/ui-enhancement
- 用户在真机上确认「可以使用 wifi 中继」，并推了自己写的界面分支（`ff84b51`）：小屏九宫格工具箱
  （Wi-Fi、5G 信号、USB 与 TTL、电池与充电、加速清理五个新页面，`H`、`Z` 两种新记录），网页顶部状态条和首页快捷工具箱，
  新接口 `POST /api/system/drop-caches`，小屏状态不变时不重绘。
- 合并前发现并修复（在用户的提交之上另加提交）：
  - agent 编译不过：调用了不存在的 `restore()`，改成 `close()`；格式化。
  - 小屏发的是 `tool wifi_toggle 2g 1` 这类指令，agent 解析的是 `wifi 2g 1`，所有按钮都会被拒绝；agent 改为按小屏的格式解析。
  - 字段和取值与现有接口对不上：Wi-Fi 是 `radio2_disabled`/`clients_total`；充电只认 `charging_stopped`、`charge_limit`、
    `hysteresis`；网络模式是固件的 `WL_AND_5G`、`Only_5G`、`LTE_AND_5G`、`Only_LTE`（网页首页也改了）；USB 只有 rndis/ecm/ncm，
    当前模式是 `active_mode`，接口没有 ADB 开关（小屏去掉了 ADB 按钮）；信号取固件原始的 `nr5g_*`/`lte_*`；电池取 `voltage_mv`、`current_ma`。
  - 负载：`H` 记录要十几次固件和系统调用，原来跟着状态每 2 秒读一次，改成每 10 秒一次，工具箱操作后立即刷新。
  - 网页没有调用清理缓存接口，契约检查不过：首页工具箱加了「释放内存缓存」按钮。
- 文档：SCREEN（工具箱页面、`H`/`Z`、`tool …` 操作、菜单最多九项）、DASHBOARD、AGENT、README。端到端 S8、S11 跟着更新。

### 2026-10-16 — 完整端到端复测、工具箱逐个按钮测试、资源占用实测与优化
- 用户要求「重新进行完善的 e2e 测试，并且检查所有功能是否正常，新版本是否优化了系统的占用，更流畅」。
- 实测（模拟设备、x86 release 版、每段 2 分钟；外部命令数与平台无关，CPU 只作相对比较）：

  | 场景（每分钟外部命令 / agent CPU） | v1.1.2 | 合并后的 main（11fdef9） | 本次优化后 |
  | --- | --- | --- | --- |
  | 空闲 | 4 / 0.30% | 4 / 0.30% | 4 / 0.28% |
  | 网页首页开着 | 35 / 0.54% | 35 / 0.55% | 35 / 0.55% |
  | 小屏开着，默认菜单 | 22 / 0.86% | 94 / 1.15% | 22 / 0.82% |
  | 小屏开着，工具箱菜单 | — | 94 / 0.82% | 42 / 0.65% |

  合并后的 main 比 v1.1.2 更费：工具箱数据不管菜单里有没有工具箱都每 10 秒读一次，每次十几个命令。
- 优化：
  - agent：只有菜单（或启动页）里有工具箱页面时才读 `H`；Wi-Fi 一项只读一次 `uci show wireless`，在线数用已有的客户端列表，
    USB 模式读 sysfs（原来借用完整的 Wi-Fi 状态和 USB 状态接口，每次 9 个命令）；TTL 每分钟读一次，按 TTL 按钮后立即重读。
  - 小屏：每次轮询先生成页面 HTML（便宜），与上一帧完全相同就不排版、不渲染、不刷屏。分支里比较整个状态结构体的做法几乎从不生效
    （CPU、内存、网速每次都变），改为比较 HTML。v1.1.2 每次轮询都排版渲染一次，只省掉了刷屏。
    回放实测（录下的 60 次轮询）：v1.1.2 每页 60 次，分支 55 次，现在大多数页面 1–2 次；概览和加速清理页显示 CPU、内存，仍为 53–55 次。
    每次排版渲染在 x86 上约 6 毫秒，设备上会慢几倍。
  - 网页：没有变化。首次加载 120.7 → 123.7 KiB（gzip），首页每分钟 20 个请求、代理页 28 个，两版相同。
- 端到端新增 S17（`tests/s17_toolbox.py`，模拟固件 `fw/`）：经屏幕套接字逐个按工具箱按钮，核对发给固件的调用和 `H` 的即时变化；
  默认菜单下不读工具箱数据。S11 加了顶部状态条和首页工具箱（TTL、释放缓存）的检查。负载测量脚本 `tests/perf_load.py`。

### 2026-10-17 — 登录页「高级后台」、渲染图更新
- 用户要求「后台登录只写高级后台，不要带clash，登录的时候」：登录页副标题、浏览器标签标题、侧边栏顶部都改为「高级后台」
  （英文界面「Advanced admin」；「Advanced dashboard」在侧边栏会被截断），演示模式提示去掉了 OpenUI/WireGuard/Clash 字样。
  标签标题随语言切换（`applyLang` 里设置）。S11 检查登录页只显示这几个字、不含 Clash/WireGuard。
- 网页 DevUI 页「USB 与 TTL」的说明去掉了「ADB 开关」（接口没有这个开关，小屏上已去掉）。
- 渲染图（`docs/images/`，英文）全部按当前界面重新生成，并在文档里引用：
  - 小屏 14 张（预览构建渲染）：菜单、九宫格、概览、WireGuard、设备分流、配置、eSIM、中继和中继认证、五个工具箱页面，放在 SCREEN.md；
  - 网页 9 张（演示数据 `mock_agent.py --wireguard-demo`，1280×900）：登录页、首页、Wi-Fi 中继、DevUI 两张放在 DASHBOARD.md 和 WIFI-UPLINK.md，
    WireGuard 四张（其中一张深色模式）放在 WIREGUARD.md；
  - 安装器线框图重新生成后逐字节相同，沿用 `installer-v1.1`。
- 用户还要求「删除合作者claude，不要带任何ai标识」：核对了 GitHub 贡献者（acloudpeng、czwstc）、协作者（只有 acloudpeng）、
  所有提交的作者和提交信息（没有 Co-Authored-By）、发布页作者（github-actions）、仓库文件（只有上游账号名 jesther-ai），都没有 Claude。
  本次提交照旧只署 acloudpeng。唯一沾边的是早期提交 `41e5d63` 的说明里的旧文件名 `AI-OPERATIONS.md`（2026-10-10 已改名为 OPERATIONS.md），
  改它们要再改写一次历史、移动所有标签，没有动。

### 2026-10-17 — 1.1.3
- 用户：「提交推送，发布 v1.1.3」。内容：九宫格工具箱与网页状态条（用户的界面，含合并时的修复）、资源占用优化、登录页「高级后台」、渲染图更新。
- 版本号改为 1.1.3（agent、网页、安装器和各自的锁文件）。发布说明 `docs/releases/v1.1.3.md` 用三种语言。补丁版本沿用 v1.1 的线框图。
