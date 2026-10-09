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
| **P0** | 解锁（`zunlock.py --dry-run` 先行，需 backup-key 后缀）→ OpenUI `setup.sh` 装基础 + SSH → 只读探测脚本 → `docs/B31-PROBE.md` | 是（**等后缀**） | **完成（2026-10-10 实机）**：B31 解锁成功，suffix 有效；agent + SSH + 网页 + mihomo + geo 已部署，重启自启验证通过。过程见 `docs/AI-OPERATIONS.md` |
| **P1** | mihomo 交叉编译（CI + 本地 Docker）、`prepare-mihomo.py`、sha256 固定；实机基准（内存/CPU/吞吐） | 基准需要 | 构建完成；**内核已在实机部署并校验 sha256**，吞吐基准待测 |
| **P2** | 从 `wireguard.rs` 抽出 `egress_policy`（设备、标记位、路由表参数化）；WG 行为不变；互斥切换事务与回滚 | 否 | 完成（互斥切换在 P3 实现） |
| **P3** | `clash.rs`：进程监管、订阅子模块、配置合成、Unix socket 控制客户端、策略应用、revision/409 | 否（mock） | 完成（netns 数据面测试通过）；**`/api/clash` 在实机返回 `core_installed/tun_supported/geodata` 正常**，待配订阅 |
| **P4** | 网页「代理」分组：概览 / 订阅 / 节点组 / 设备分流 / 连接 / 日志 / 高级设置；`api.ts`、`mock_agent.py`、契约检查 | 否 | 完成（mock + 浏览器走查）；**网页 8080 实机可访问** |
| **P5** | `deploy-components.py --mihomo`、`transaction.sh` 快照目标、geo 数据打包 | 否 | **完成（实机部署成功，事务快照已生成）** |
| **P6** | B31 实机验证：吞吐、DNS/IPv6/kill-switch 泄漏、防火墙重载/重启/WAN 重连恢复、72 h 内存浸泡、WG↔Clash 切换 | 是 | 进行中：**部署 + 重启持久性已验证**；吞吐/泄漏/浸泡待测 |
| **P7** | DevUI 小屏：后端状态、Clash 开关/模式/节点 | 否（预览） | 可选 |
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
   `docs/AI-OPERATIONS.md` §6.2 抓取指纹。
9. **Windows 主机执行 `deploy-components.py` 需包装**：管道 `selectors`、
   `os.killpg`、CRLF 双重转换、宿主机无 `sh -n`（见 `docs/AI-OPERATIONS.md` §7）。
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
