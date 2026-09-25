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
4. **不用 mihomo 的 auto-route / auto-redirect**：固件是 fw3 + iptables-legacy（B27 数据，B31 待确认），auto-redirect 依赖 nftables。
   改用 WireGuard 分叉已验证的方案：`br-lan` 上按 MAC 打 fwmark → `ip rule` → 独立路由表 → `ou-clash0`（TUN，`auto-route: false`）。
   kill-switch、HOLD/RELEASE、revision/409、30 s 看门狗全部复用。
5. **mihomo 由 agent 作为子进程监管**，不新增 rc.local 行（遵守 SAFETY.md）。参考原作 Tailscale 模块：等 WAN、退避重启、WAN 重连事件、wakelock、`GOMEMLIMIT`。
6. **控制接口**走 `external-controller-unix` + 随机 secret，不在局域网开放端口；网页只经 agent 的 `/api/clash/*` 访问。
7. **订阅 = URL 下载完整 Clash YAML**：
   - agent 调用设备自带 `/usr/bin/curl` 下载（不给 agent 引入 TLS 依赖）；UA 默认 `clash.meta`，30 s，上限 8 MiB。
   - 读取 `subscription-userinfo`（流量/到期）、`profile-update-interval`（自动更新）、`content-disposition`（默认名）。
   - base64 节点列表第一版直接报错。
   - 合成：保留 `proxies / proxy-groups / rules / *-providers / dns 上游 / sniffer`；
     强制覆盖 `tun`、端口（仅 127.0.0.1 mixed-port）、`allow-lan: false`、`dns.listen/enhanced-mode(fake-ip)/fake-ip-range`、
     `external-controller-unix + secret`、删除 `external-ui / iptables / interface-name / routing-mark`、
     `find-process-mode: off`、`geodata-loader: memconservative`、`profile.store-selected: true`。
   - `mihomo -t` 校验通过才替换，失败保留上一份可用配置；`PUT /configs` 热重载。
   - 手动更新、按间隔自动更新、可选"通过代理更新"（curl 走 127.0.0.1 mixed-port）。
   - 最多 5 个订阅；订阅 URL 视为密钥，API 只返回打码值。
   - geo 数据随部署包推送（国内首启无法从 GitHub 下载）；`geox-url` 可配镜像。
8. **DNS / IPv6**：被选设备的 53 端口 DNAT 到 mihomo DNS；IPv6 先阻断（与 WG 一致）。未选设备与路由器自身不受影响，dnsmasq 不做持久修改。

## 阶段

| 阶段 | 内容 | 依赖设备 | 状态 |
|---|---|---|---|
| **P0** | 解锁（`zunlock.py --dry-run` 先行，需 backup-key 后缀）→ OpenUI `setup.sh` 装基础 + SSH → 只读探测脚本 → `docs/B31-PROBE.md` | 是（**等后缀**） | 探测脚本可先写 |
| **P1** | mihomo 交叉编译（CI + 本地 Docker）、`prepare-mihomo.py`、sha256 固定；实机基准（内存/CPU/吞吐） | 基准需要 | 进行中 |
| **P2** | 从 `wireguard.rs` 抽出 `egress_policy`（设备、标记位、路由表参数化）；WG 行为不变；互斥切换事务与回滚 | 否 | 待开始 |
| **P3** | `clash.rs`：进程监管、订阅子模块、配置合成、Unix socket 控制客户端、策略应用、revision/409 | 否（mock） | 待开始 |
| **P4** | 网页「代理」分组：概览 / 订阅 / 节点组 / 设备分流 / 连接 / 日志 / 高级设置；`api.ts`、`mock_agent.py`、契约检查 | 否 | 待开始 |
| **P5** | `deploy-components.py --mihomo`、`transaction.sh` 快照目标、geo 数据打包 | 否 | 待开始 |
| **P6** | B31 实机验证：吞吐、DNS/IPv6/kill-switch 泄漏、防火墙重载/重启/WAN 重连恢复、72 h 内存浸泡、WG↔Clash 切换 | 是 | 待开始 |
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
5. B31 解锁流程未经上游验证（上游只验证 HK B04）。
6. mihomo 与屏幕程序为 GPL-3.0，分发需给出源码 commit。
7. 用上游 OpenUI 安装器更新会覆盖本仓库新增内容。
