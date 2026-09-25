import type { ClashDnsMode, ClashLogLevel, ClashProxyMode, ClashStack } from '../../types'
import { formatUptime } from '../../format'
import { Field, Segmented, Select, Toggle } from '../../ui/controls'
import { Card, Chip, Stat } from '../../ui/primitives'
import { useClash } from './ClashContext'

export default function OverviewTab() {
  const { status, error, busy, update } = useClash()
  const active = status?.subscriptions.find(s => s.id === status.active_subscription)
  const ready = !!status?.core_installed && !!status?.tun_supported && !!active?.downloaded
  const summary = status?.connected ? 'Running' : status?.enabled ? status.error ? 'Needs attention' : 'Starting' : 'Off'

  return (
    <div className="space-y-4">
      <Card title="Clash" action={<Chip tone={status?.connected ? 'ok' : status?.enabled ? 'warn' : 'default'}>{summary}</Chip>}>
        <div className="space-y-4">
          <div className="flex items-start justify-between gap-4">
            <div>
              <p className="text-[13px] font-medium text-ink">Rule-based proxy for your connected devices</p>
              <p className="mt-1 text-xs text-ink2">Traffic from the chosen devices goes through the Clash core and follows the active subscription’s rules. Only one of Clash and WireGuard can be on.</p>
            </div>
            <Toggle label="Enable Clash" checked={status?.enabled ?? false} disabled={busy || !status || (!status.enabled && !ready)}
              onChange={v => void update({ enabled: v }, v ? 'Unable to start Clash' : 'Unable to stop Clash')} />
          </div>
          {(error || status?.error) && <p role="alert" className="rounded-lg bg-danger/10 p-3 text-[13px] text-danger">{error || status?.error}</p>}
          {!status && <p className="text-sm text-ink2">Loading Clash status…</p>}
          {status && <>
            <div className="flex flex-wrap gap-2">
              <Chip tone={status.core_installed ? 'ok' : 'warn'}>Core {status.core_installed ? 'installed' : 'missing'}</Chip>
              <Chip tone={status.tun_supported ? 'ok' : 'warn'}>TUN {status.tun_supported ? 'ready' : 'unavailable'}</Chip>
              {status.wireguard_enabled && <Chip tone="accent">WireGuard is on</Chip>}
              {!active && <Chip tone="warn">No subscription selected</Chip>}
              {active && !active.downloaded && <Chip tone="warn">Subscription not downloaded</Chip>}
            </div>
            <div className="grid grid-cols-2 gap-4 sm:grid-cols-4">
              <Stat label="Subscription" value={active?.name ?? '—'} sub={active ? `${active.proxies} nodes · ${active.rules} rules` : undefined} />
              <Stat label="Uptime" value={status.running ? formatUptime(status.uptime) : '—'} />
              <Stat label="Memory" value={status.running ? `${(status.memory_kib / 1024).toFixed(0)} MB` : '—'} />
              <Stat label="Core" value={status.version || '—'} />
            </div>
          </>}
        </div>
      </Card>

      {status && <Card title="Proxy mode">
        <div className="space-y-3">
          <Segmented<ClashProxyMode> disabled={busy} value={status.proxy_mode}
            options={[{ value: 'rule', label: 'Rule' }, { value: 'global', label: 'Global' }, { value: 'direct', label: 'Direct' }]}
            onChange={v => { if (v !== status.proxy_mode) void update({ proxy_mode: v }, 'Unable to change proxy mode') }} />
          <p className="text-xs text-ink2">
            {status.proxy_mode === 'rule' && 'Rule: the subscription’s rules decide which traffic is proxied and which goes direct.'}
            {status.proxy_mode === 'global' && 'Global: all traffic from proxy devices uses the node chosen in the GLOBAL group.'}
            {status.proxy_mode === 'direct' && 'Direct: the core stays in the path but sends everything straight out. Useful to rule out node problems.'}
          </p>
          <p className="text-xs text-ink3">The mode applies instantly without restarting the core. Reloading a subscription resets it to this setting.</p>
        </div>
      </Card>}

      {status && <Card title="Advanced">
        <div className="grid gap-4 sm:grid-cols-3">
          <Field label="TUN stack" hint="System is fastest; gVisor is more compatible. Applying briefly pauses proxy traffic.">
            <Select aria-label="TUN stack" value={status.stack} disabled={busy} onChange={e => void update({ stack: e.target.value as ClashStack }, 'Unable to change TUN stack')}>
              <option value="system">System</option>
              <option value="gvisor">gVisor</option>
              <option value="mixed">Mixed (TCP system, UDP gVisor)</option>
            </Select>
          </Field>
          <Field label="DNS mode" hint="Fake-IP avoids DNS leaks and is recommended. Applying briefly pauses proxy traffic.">
            <Select aria-label="DNS mode" value={status.dns_mode} disabled={busy} onChange={e => void update({ dns_mode: e.target.value as ClashDnsMode }, 'Unable to change DNS mode')}>
              <option value="fake-ip">Fake-IP</option>
              <option value="redir-host">Redir-host</option>
            </Select>
          </Field>
          <Field label="Log level" hint="Info or debug shows each connection in Logs.">
            <Select aria-label="Log level" value={status.log_level} disabled={busy} onChange={e => void update({ log_level: e.target.value as ClashLogLevel }, 'Unable to change log level')}>
              <option value="silent">Silent</option>
              <option value="error">Error</option>
              <option value="warning">Warning</option>
              <option value="info">Info</option>
              <option value="debug">Debug</option>
            </Select>
          </Field>
        </div>
        <p className="mt-3 text-xs text-ink3">IPv6 internet and IPv6 DNS are blocked for proxy devices so nothing bypasses the core. Router management always stays reachable.</p>
      </Card>}
    </div>
  )
}
