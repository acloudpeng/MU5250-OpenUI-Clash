// Shared domain types for the agent API.

export interface CarrierComponent {
  label: string // "PCC", "SCC0", "SCC1", etc.
  band: string // "B8", "n78"
  pci: number
  earfcn: number
  bandwidth: string // "10 MHz"
  freq?: number // MHz, calculated from EARFCN
  rsrp?: number
  rsrq?: number
  sinr?: number
  rssi?: number
  ul_configured?: boolean
  active?: boolean
}

export interface SignalInfo {
  type?: string
  carrier?: string
  signal_bars?: number
  cell_id?: string
  lte_carriers: CarrierComponent[]
  nr_carriers: CarrierComponent[]
  net_select?: string
  lte_band_lock?: number[]
  nr_band_lock?: number[]
  raw_lte_band_lock?: string
  raw_nr_band_lock?: string
  rsrp?: number
  band?: string
}

export interface BatteryInfo {
  percent: number
  charging: boolean
  voltage_mv?: number
  temperature_c?: number
  current_ma?: number
}

/** Live WAN throughput, in **bytes** per second (`formatSpeed` converts to bits). */
export interface SpeedInfo {
  rx_bps: number
  tx_bps: number
  max_rx_bps: number
  max_tx_bps: number
}

export interface DeviceInfo {
  model: string
  firmware?: string
  uptime_secs?: number
  load_avg?: number[]
}

export interface WanInfo {
  connected: boolean
  ipv4?: string
  ipv6?: string
  gateway?: string
  dns?: string[]
  apn?: string
}

export interface Wan6Info {
  connected: boolean
  ipv6?: string
  prefix?: string
  dns?: string[]
}

export interface Client {
  mac: string
  ip?: string
  hostname?: string
  medium?: 'wifi' | 'usb-c' | 'ethernet' | 'wired'
  medium_detail?: 'wifi_2ghz' | 'wifi_5ghz' | 'usb_c' | 'ethernet'
  interface?: string
  wifi_band?: string
  signal_dbm?: number
  tx_bitrate_mbps?: number
  rx_bitrate_mbps?: number
  expected_throughput_mbps?: number
  connected_secs?: number
  wired_link_mbps?: number
}

export interface CpuInfo {
  overall: number
  cores: number[]
}

export type UsbMode = 'ecm' | 'rndis' | 'ncm'

export interface UsbModeCapability {
  mode: UsbMode
  supported: boolean
  experimental: boolean
  function?: string
  note?: string
}

export interface UsbLink {
  negotiated?: string
  negotiated_label?: string
  negotiated_mbps?: number
  max?: string
  max_label?: string
  max_mbps?: number
  at_full_speed?: boolean
}

export interface UsbStatus {
  active_mode: UsbMode | null
  default_mode?: UsbMode
  link?: UsbLink
  ncm_persist_on_boot?: boolean
  supported_modes: string[]
  experimental_modes?: string[]
  mode_capabilities?: UsbModeCapability[]
  composition_functions?: string[]
  configfs?: { present?: boolean; ncm?: boolean; gsi_ecm?: boolean; gsi_rndis?: boolean }
  bridge?: { name?: string; members?: string[] }
  interfaces?: { ecm0?: boolean; rndis0?: boolean; ncm0?: boolean; ncm_ifname?: string | null }
  usb_ids?: { vendor?: string | null; product?: string | null }
  ncm_last_error?: string
  connect?: number
  typec_cc?: string
}

export interface MemInfo {
  total_kb: number
  used_kb: number
  free_kb: number
  usage_pct: number
}

export interface WifiBand {
  ssid?: string
  enabled: boolean
  channel?: number
  bandwidth?: string
  configuredChannel?: string
  configuredBandwidth?: string
  bandwidthOptions?: string[]
  supportedStandards?: string
  actualChannel?: number
  actualBandwidth?: string
  password?: string
  security?: string
  hidden: boolean
  clients?: number
}

export interface WifiAll {
  band_2g: WifiBand
  band_5g: WifiBand
  guest_ssid?: string
  master_supported: boolean
  master_enabled: boolean
  wifi6_supported: boolean
  wifi6_enabled?: boolean
  wifi7_supported: boolean
}

export interface DnsConfig {
  primary: string
  secondary: string
  ipv6_primary?: string
  ipv6_secondary?: string
}

export interface LanConfig {
  ipaddr: string
  netmask: string
  dhcp_enabled: boolean
  dhcp_start: string
  dhcp_end: string
  lease_seconds: number
}

export interface ModemCapabilities {
  network_modes: { value: string; label: string }[]
  lte_bands: number[]
  nr_sa_bands: number[]
  nr_nsa_band_lock_supported: boolean
}

export interface ThermalInfo {
  cpu_temp_c?: number
}

export interface ThermalAll {
  available: boolean
  cpu_0?: number
  cpu_1?: number
  cpu_2?: number
  cpu_3?: number
  modem?: number
  modem_ss0?: number
  modem_ss1?: number
  modem_ss2?: number
  battery?: number
  usb?: number
  eth_phy?: number
  pmic?: number
  xo_therm?: number
  pa?: number
  sdr?: number
}

export interface BatteryBspInfo {
  available: boolean
  online: boolean | null
  low_power: boolean | null
  using_hw_fg_chip: boolean | null
  time_to_full_mins: number | null
  time_to_empty_mins: number | null
}

export interface BatteryDetail {
  available: boolean
  capacity: number | null
  status: string | null
  voltage_mv: number | null
  voltage_max_mv: number | null
  voltage_ocv_mv: number | null
  current_ma: number | null
  power_mw: number | null
  temperature_c: number | null
  charge_type: string | null
  health: string | null
  cycle_count: number | null
  charge_counter_mah: number | null
  charge_full_mah: number | null
  charge_full_design_mah: number | null
  time_to_full_secs: number | null
  time_to_empty_secs: number | null
}

export interface ChargeControlState {
  last_error?: string | null
  available: boolean
  battery_available: boolean
  charger_available: boolean
  charging_stopped: boolean | null
  battery_status: string | null
  capacity: number | null
  charge_limit_enabled: boolean
  charge_limit: number
  hysteresis: number
  manual_override: boolean
}

export interface ApnProfile {
  profilename: string
  wanapn: string
  username: string
  password: string
  pdpType: number
  pppAuthMode: number
  profileId: string
  isEnable: boolean
}

export interface SimInfo {
  iccid?: string
  imsi?: string
  state?: string
  mcc?: string
  mnc?: string
}

export interface UsagePeriod {
  rx_bytes: number
  tx_bytes: number
  time_secs: number
}

export interface DataUsage {
  day: UsagePeriod
  month: UsagePeriod
  cycle?: UsagePeriod
  since_power_on?: UsagePeriod
  total: UsagePeriod
  reset_day?: number
  reset_enabled?: boolean
  clear_date_record?: string
  next_clear_date?: string
}

export interface SmsMessage {
  id: number
  /** Sender (inbox) or recipient (sent) number. */
  number: string
  content: string
  date?: string
  /** Firmware tags: 0=received/read, 1=received/unread, 2=sent, 3=failed, 4=draft. */
  tag: number
  /** Firmware storage: 1=native/NV device storage. */
  mem_store?: number
}

export interface SmsCapabilities {
  available: boolean
  ready: boolean
  object: string
  storage?: string
  reason?: string
}

/** One row of `GET /api/system/top` — mirrors agent/src/system.rs::ProcessEntry. */
export interface ProcessInfo {
  pid: number
  name: string
  cpu_pct: number
  rss_kb: number
  state: string
  /** True for daemons on the agent's kill-safe bloat allowlist. */
  is_bloat: boolean
}

/** `GET /api/system/top` — mirrors agent/src/system.rs::ProcessListResult. */
export interface ProcessListResult {
  processes: ProcessInfo[]
  total_count: number
  bloat_count: number
  bloat_cpu_pct: number
  bloat_rss_kb: number
}

export interface KilledProcess {
  pid: number
  name: string
}

/** `POST /api/system/kill-bloat` — mirrors agent/src/system.rs::KillBloatResult. */
export interface KillBloatResult {
  killed: KilledProcess[]
  skipped: KilledProcess[]
  freed_rss_kb: number
}

export interface LoggerStatus {
  last_error?: string | null
  max_bytes?: number
  flush_interval_secs?: number
  running: boolean
  samples?: number
  events?: number
  elapsed_secs: number
  duration_secs: number
  interval_secs: number
}

export interface LoggerDownload {
  csv: string
}

export interface TtlStatus {
  active?: boolean
  ipv6_active?: boolean
  ttl_value?: number
}

export interface AtSendResult {
  command?: string
  response: string
  port?: string
  elapsed_ms?: number
}

/** One merged poll of /api/dashboard — the home screen's single request. */
export interface SourceFreshness {
  sampled_at_ms: number | null
  age_ms: number | null
  ttl_ms: number
  stale: boolean
  error: string | null
}
export interface HomeData {
  sources?: Record<string, SourceFreshness>
  charge_control_error?: string | null
  signal: SignalInfo | null
  battery: BatteryInfo | null
  speed: SpeedInfo | null
  device: DeviceInfo | null
  wan: WanInfo | null
  wan6: Wan6Info | null
  cpu: CpuInfo | null
  memory: MemInfo | null
  usage: DataUsage | null
  thermal: ThermalInfo | null
}

/** `GET/PUT /api/proxy` — mirrors agent/src/proxy.rs: at most one backend is on. */
export type ProxyBackendId = 'wireguard' | 'clash'
export type ProxyChoice = 'off' | ProxyBackendId
export interface ProxyBackend {
  id: ProxyBackendId
  /** Saved as on. */
  enabled: boolean
  revision: number
  /** Can be turned on; otherwise `reason`, an agent message, says why not. */
  ready: boolean
  reason: string | null
  /** The last apply error. */
  error: string | null
  /** Clash only: the core runs. */
  running?: boolean
}
export interface ProxyStatus {
  active: ProxyChoice
  /** Both saved as on; WireGuard is the one in effect. */
  conflict: boolean
  backends: ProxyBackend[]
}
export interface ProxySwitch {
  backend: ProxyChoice
  /** Revision of the backend being turned on, or of the one being turned off. */
  expected_revision: number
  /** For off: the backend shown as on; the agent answers 409 if another one is on. */
  active?: ProxyBackendId
}

export interface WireGuardProfile {
  id: number
  name: string
  endpoint: string
  address: string
}
export type WireGuardProfileAction =
  | { action: 'save'; name: string; config_text: string }
  | { action: 'activate'; id: number }
  | { action: 'rename'; id: number; name: string }
  | { action: 'delete'; id: number }
export interface WireGuardStatus {
  profiles: WireGuardProfile[]
  active_profile: number | null
  max_profiles: number
  revision: number
  enabled: boolean
  mode: 'all' | 'selected'
  macs: string[]
  has_config: boolean
  kernel_supported: boolean
  tools_installed: boolean
  interface_up: boolean
  connected: boolean
  latest_handshake: number
  rx_bytes: number
  tx_bytes: number
  error: string | null
  endpoint: string | null
  address: string | null
  dns: string | null
  mtu: number | null
  ipv6_policy: string
}
/** Turning the tunnel on or off goes through `PUT /api/proxy`. */
export interface WireGuardUpdate {
  expected_revision?: number
  profile?: WireGuardProfileAction
  mode?: 'all' | 'selected'
  macs?: string[]
  config_text?: string
}

export interface ClashUsage {
  upload: number
  download: number
  total: number
  expire: number
}
export interface ClashSubscription {
  id: number
  name: string
  /** Scheme and host only; the full URL never leaves the device. */
  url: string
  user_agent: string
  interval_hours: number
  via_proxy: boolean
  updated_at: number
  usage: ClashUsage | null
  proxies: number
  groups: number
  rules: number
  providers: number
  error: string | null
  downloaded: boolean
}
export type ClashSubscriptionAction =
  | { action: 'add'; url: string; name?: string; user_agent?: string; interval_hours?: number; via_proxy?: boolean }
  | { action: 'edit'; id: number; url?: string; name?: string; user_agent?: string; interval_hours?: number; via_proxy?: boolean }
  | { action: 'activate'; id: number }
  | { action: 'delete'; id: number }
export type ClashProxyMode = 'rule' | 'global' | 'direct'
export type ClashLogLevel = 'silent' | 'error' | 'warning' | 'info' | 'debug'
export type ClashStack = 'system' | 'gvisor' | 'mixed'
export type ClashDnsMode = 'fake-ip' | 'redir-host'
export interface ClashStatus {
  revision: number
  enabled: boolean
  mode: 'all' | 'selected'
  macs: string[]
  proxy_mode: ClashProxyMode
  log_level: ClashLogLevel
  stack: ClashStack
  dns_mode: ClashDnsMode
  subscriptions: ClashSubscription[]
  active_subscription: number | null
  max_subscriptions: number
  core_installed: boolean
  /** Deployed geo databases (geoip.metadb, GeoSite.dat, ASN.mmdb). */
  geodata: string[]
  tun_supported: boolean
  running: boolean
  pid: number | null
  uptime: number
  version: string
  memory_kib: number
  interface_up: boolean
  connected: boolean
  error: string | null
  ipv6_policy: string
}
/** Turning Clash on goes through `PUT /api/proxy`; `enabled: false` only resolves a conflict. */
export interface ClashUpdate {
  expected_revision: number
  enabled?: false
  mode?: 'all' | 'selected'
  macs?: string[]
  devices?: { mac: string; selected: boolean }[]
  proxy_mode?: ClashProxyMode
  log_level?: ClashLogLevel
  stack?: ClashStack
  dns_mode?: ClashDnsMode
  subscription?: ClashSubscriptionAction
}
export interface ClashGroup {
  name: string
  type: string
  now: string | null
  all: string[]
  hidden: boolean
  selectable: boolean
}
export interface ClashNode {
  type: string
  delay: number | null
  udp: boolean | null
  alive: boolean | null
}
export interface ClashProxies {
  groups: ClashGroup[]
  nodes: Record<string, ClashNode>
}
export interface ClashConnection {
  id: string
  network: string
  source: string
  host: string
  destination: string
  port: string
  sniff_host: string
  rule: string
  rule_payload: string
  chains: string[]
  upload: number
  download: number
  start: string
}
export interface ClashConnections {
  upload_total: number
  download_total: number
  memory: number
  connections: ClashConnection[]
}

/** `GET /api/clash/diagnostics` — mirrors agent/src/clash_diag.rs. */
export type ClashCheckStatus = 'pass' | 'warn' | 'fail' | 'skip'
export type ClashVerdict = 'clash_off' | 'core_down' | 'not_routed' | 'nodes' | 'no_devices' | 'ok'
export interface ClashDiagClient {
  ip: string
  mac: string
  name: string | null
  interface: string | null
  proxied: boolean
  /** The core has connections from this address. */
  active: boolean
}
/** Each check's data, by check id. */
export interface ClashCheckData {
  /** Device clock minus a domestic site's HTTP Date (positive = ahead); ntp: the profile has the core's NTP block. */
  /** beijing: B31's system clock keeps Beijing time while the core keeps NTP time. */
  clock: { skew_s: number | null; ntp: boolean; beijing?: boolean }
  core: { running: boolean; supervised: boolean; version: string | null }
  dns: { domestic: boolean; foreign: boolean }
  /** Main selector group, the node it resolves to, its delays, and members answering a group test. */
  nodes: { group: string; node: string; https_ms: number | null; http_ms: number | null; answered: number; total: number }
  /** HTTP status of fetches by the router through the core's mixed port; null = failed. */
  fetch: { domestic: number | null; foreign: number | null }
  devices: { mode: 'all' | 'selected'; proxied: number; total: number; clients: ClashDiagClient[] }
  /** The check's `detail` holds the raw `ip route get` lines and `ip rule show`. */
  route: { client: string; tun: boolean; dns_tun: boolean }
  firewall: {
    missing: string[]
    /** Owned hooks with a firmware rule above them. */
    displaced: string[]
    /** The transition guard was left in place (blocks LAN forwarding). */
    held: boolean
    /** Packets the kill switch rejected: marked traffic not routed into the TUN. */
    rejected: number
    ipa_acceleration: boolean | null
    rp_filter: number | null
  }
}
export type ClashCheckId = keyof ClashCheckData
/** One check; a skipped one may carry no data. */
export type ClashCheck = {
  [K in ClashCheckId]: { id: K; status: ClashCheckStatus; data: Partial<ClashCheckData[K]>; detail: string }
}[ClashCheckId]
export interface ClashDiagnostics {
  running: boolean
  /** Device clock (epoch seconds), which may be skewed; null before the first run. */
  started_at: number | null
  finished_at: number | null
  duration_ms?: number
  verdict: ClashVerdict | null
  checks: ClashCheck[]
}

/** `GET/POST /api/clash/core` — mirrors agent/src/clash_core.rs; times are device clock seconds.
 * pinned: the FlClash build the deploy script installs; official: a MetaCubeX release installed here. */
export type ClashCoreSource = 'pinned' | 'official' | 'uploaded' | 'unknown'
export interface ClashCoreBuild {
  /** A release tag (v1.19.32) or another build's token. */
  version: string
  sha256: string
  source: ClashCoreSource
}
/** The last release check, cached by the agent for 10 minutes. */
export interface ClashCoreRelease {
  tag: string
  /** GitHub's ISO time (not the device clock). */
  published_at: string
  checked_at: number
  asset: string
  size: number
  digest: string
  url: string
}
export type ClashCoreJobKind = 'check' | 'update' | 'rollback' | 'upload' | 'geodata'
export type ClashCoreStep = 'checking' | 'receiving' | 'downloading' | 'verifying' | 'extracting' | 'testing' | 'installing' | 'restarting'
/** The running job, or the last one until the next starts. */
export interface ClashCoreJob {
  kind: ClashCoreJobKind
  state: 'running' | 'done' | 'failed'
  step: ClashCoreStep
  /** Progress of a downloading or receiving step; `total` is 0 when unknown. */
  bytes: number
  total: number
  error: string | null
  started_at: number
  finished_at: number | null
}
export interface ClashCore {
  installed: ClashCoreBuild | null
  previous: ClashCoreBuild | null
  latest: ClashCoreRelease | null
  job: ClashCoreJob | null
  /** Available on /data; 0 when unknown. */
  free_bytes: number
  /** The last download from GitHub failed: suggest a mirror. */
  mirror_hint: boolean
}
/** auto: through the running core, else direct. mirror: the large files from an https:// prefix. */
export type ClashCoreRoute = 'auto' | 'proxy' | 'direct' | 'mirror'
export type ClashCoreAction =
  | { action: 'check'; route?: Exclude<ClashCoreRoute, 'mirror'> }
  | { action: 'update'; tag: string; route?: ClashCoreRoute; mirror?: string }
  | { action: 'rollback' }
  | { action: 'geodata'; route?: ClashCoreRoute; mirror?: string }

export type DevUIPageId = 'overview' | 'proxy' | 'wireguard' | 'clash' | 'esim' | 'uplink' | 'profiles' | 'devices'
export interface DevUISettings {
  revision: number
  shortcut_enabled: boolean
  /** Two fingers held still on the lit stock UI open DevUI. Absent from older agents. */
  touch_enabled?: boolean
  /** DevUI opens once the stock UI has started after a boot. */
  open_at_boot?: boolean
  /** When idle: true turns the screen off and stays in DevUI, false returns to the stock UI. */
  idle_sleep?: boolean
  brightness: number
  idle_seconds: number
  home_page: 'menu' | DevUIPageId
  menu_items: DevUIPageId[]
  /** Screen language; set by the panel's language switch through screenLanguageSet. */
  language?: 'zh' | 'en'
}
export interface ScreenStatus {
  settings_revision: number
  shortcut: { available: boolean; message: string }
  /** The two-finger hold listener; absent from older agents. */
  touch?: { available: boolean; message: string }

  available: boolean
  verified: boolean
  running: boolean
  state: string
}

// ── eSIM (agent/src/esim.rs) ──────────────────────────────────────────────────

export interface EsimProfile {
  iccid: string
  aid: string
  enabled: boolean
  name: string
  provider: string
  nickname: string
  /** operational, test or provisioning */
  class: string
}
export interface EsimNotification {
  seq: number
  /** install, enable, disable or delete */
  operation: string
  iccid: string
  /** The SM-DP+ the notification goes to. */
  address: string
}
/** The last read of the card in the SIM tray. */
export interface EsimView {
  read_at: number
  card: 'euicc' | 'sim' | 'absent' | 'error'
  detail: string | null
  eid: string
  info: {
    default_smdp?: string | null
    root_smds?: string | null
    sgp22?: string | null
    profile_version?: string | null
    firmware?: string | null
    free_nvm?: number | null
    installed_apps?: number | null
    ci_pkids?: string[] | null
    sas?: string | null
  } | null
  profiles: EsimProfile[]
  notifications: EsimNotification[]
  /** What the firmware reports; null when it could not be read. */
  firmware: { iccid: string; state: string | null } | null
  generation: number
}
export type EsimJobKind = 'scan' | 'enable' | 'disable' | 'nickname' | 'delete' | 'download' | 'notifications'
export interface EsimJob {
  id: number
  kind: EsimJobKind
  state: 'running' | 'done' | 'failed'
  /** The agent's step (reading, radio_offline…) or lpac's (es9p_authenticate_client…). */
  step: string
  error: string | null
  result: Record<string, unknown> | null
  started_at: number
  finished_at: number | null
}
export interface EsimStatus {
  /** lpac is on the device. */
  installed: boolean
  job: EsimJob | null
  view: EsimView | null
  /** A switch the firmware has not taken up yet; a restart finishes it. */
  reboot_needed: boolean
  clash_running: boolean
}
/** auto: through the running Clash core, else direct. */
export type EsimRoute = 'auto' | 'proxy' | 'direct'
export type EsimAction =
  | { action: 'scan' }
  | { action: 'enable' | 'disable'; iccid: string }
  | { action: 'nickname'; iccid: string; nickname: string }
  | {
      action: 'download'
      activation_code?: string
      smdp?: string
      matching_id?: string
      confirmation_code?: string
      imei?: string
      route?: EsimRoute
      insecure?: boolean
    }
  | { action: 'notifications'; operation: 'process' | 'remove'; seq: number[]; route?: EsimRoute }

// ── Wi-Fi uplink / relay (agent/src/wifi_uplink.rs) ───────────────────────────

export type UplinkSecurity = 'open' | 'wpa2' | 'wpa3' | 'wpa2_wpa3'
export type UplinkState =
  | 'off' | 'unavailable' | 'connecting' | 'obtaining_ip' | 'checking' | 'online' | 'no_internet'
  | 'needs_login' | 'logging_in'

export interface UplinkNetwork {
  id: number
  ssid: string
  security: UplinkSecurity
  hidden: boolean
  has_password: boolean
}

export interface UplinkStatus {
  revision: number
  enabled: boolean
  state: UplinkState
  /** English agent message; shown through tm(). */
  reason: string | null
  ssid: string | null
  bssid: string | null
  frequency: number | null
  band: string | null
  channel: number | null
  /** The saved network the radio is joined to. */
  network_id: number | null
  signal: number | null
  ip: string | null
  prefix: number | null
  gateway: string | null
  dns: string[]
  online: boolean
  /** Device clock (B31 runs hours off): only usable together with a device "now". */
  online_since: number | null
  /** Seconds online, when the agent reports it. */
  online_for?: number | null
  /** The login page a captive portal pointed at. */
  portal_url?: string | null
  /** Seconds left in login mode. */
  login_remaining?: number | null
  rx_bytes: number
  tx_bytes: number
  networks: UplinkNetwork[]
  max_networks: number
  tools: { wpa_supplicant: boolean; udhcpc: boolean }
}

export interface UplinkSeen {
  ssid: string
  bssid: string
  frequency: number
  band: string
  channel: number | null
  signal: number
  /** null for WEP, enterprise and other networks the relay cannot join. */
  security: UplinkSecurity | null
  supported: boolean
}

/** A saved network in a save; an omitted password keeps the saved one. */
export interface UplinkNetworkEdit {
  id?: number
  ssid: string
  security: UplinkSecurity
  password?: string
  hidden: boolean
}

export interface UplinkUpdate {
  expected_revision: number
  enabled?: boolean
  networks?: UplinkNetworkEdit[]
}
