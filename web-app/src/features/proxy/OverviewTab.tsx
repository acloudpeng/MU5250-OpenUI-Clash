import type { ClashDnsMode, ClashLogLevel, ClashProxyMode, ClashStack } from '../../types'
import { formatUptime } from '../../format'
import { Field, Segmented, Select, Toggle } from '../../ui/controls'
import { Card, Chip, Stat } from '../../ui/primitives'
import { useClash } from './ClashContext'
import { t, tm } from '../../i18n'

export default function OverviewTab() {
  const { status, error, busy, update } = useClash()
  const active = status?.subscriptions.find(s => s.id === status.active_subscription)
  const ready = !!status?.core_installed && !!status?.tun_supported && !!active?.downloaded
  const summary = status?.connected ? t('Running') : status?.enabled ? status.error ? t('Needs attention') : t('Starting') : t('Off')

  return (
    <div className="space-y-4">
      <Card title="Clash" action={<Chip tone={status?.connected ? 'ok' : status?.enabled ? 'warn' : 'default'}>{summary}</Chip>}>
        <div className="space-y-4">
          <div className="flex items-start justify-between gap-4">
            <div>
              <p className="text-[13px] font-medium text-ink">{t('Rule-based proxy for your connected devices')}</p>
              <p className="mt-1 text-xs text-ink2">{t('Traffic from the chosen devices goes through the Clash core and follows the active subscription’s rules. Only one of Clash and WireGuard can be on.')}</p>
            </div>
            <Toggle label={t('Enable Clash')} checked={status?.enabled ?? false} disabled={busy || !status || (!status.enabled && !ready)}
              onChange={v => void update({ enabled: v }, v ? t('Unable to start Clash') : t('Unable to stop Clash'))} />
          </div>
          {(error || status?.error) && <p role="alert" className="rounded-lg bg-danger/10 p-3 text-[13px] text-danger">{tm(error || status?.error)}</p>}
          {!status && <p className="text-sm text-ink2">{t('Loading Clash status…')}</p>}
          {status && <>
            <div className="flex flex-wrap gap-2">
              <Chip tone={status.core_installed ? 'ok' : 'warn'}>{status.core_installed ? t('Core installed') : t('Core missing')}</Chip>
              <Chip tone={status.tun_supported ? 'ok' : 'warn'}>{status.tun_supported ? t('TUN ready') : t('TUN unavailable')}</Chip>
              <Chip tone={status.geodata.length ? 'ok' : 'warn'}>{status.geodata.length ? t('Geo data installed') : t('Geo data missing')}</Chip>
              {status.wireguard_enabled && <Chip tone="accent">{t('WireGuard is on')}</Chip>}
              {!active && <Chip tone="warn">{t('No subscription selected')}</Chip>}
              {active && !active.downloaded && <Chip tone="warn">{t('Subscription not downloaded')}</Chip>}
            </div>
            {!status.geodata.length && <p className="text-xs text-ink2">{t('GEOIP and GEOSITE rules make the core download its databases from GitHub, which often fails here. Deploy them with {flag}.').split('{flag}').flatMap((part, i) => i ? [<code key={i}>--mihomo-geodata</code>, part] : [part])}</p>}
            <div className="grid grid-cols-2 gap-4 sm:grid-cols-4">
              <Stat label={t('Subscription')} value={active?.name ?? '—'} sub={active ? `${t('{n} nodes', { n: active.proxies })} · ${t('{n} rules', { n: active.rules })}` : undefined} />
              <Stat label={t('Uptime')} value={status.running ? formatUptime(status.uptime) : '—'} />
              <Stat label={t('Memory')} value={status.running ? `${(status.memory_kib / 1024).toFixed(0)} MB` : '—'} />
              <Stat label={t('Core')} value={status.version || '—'} />
            </div>
          </>}
        </div>
      </Card>

      {status && <Card title={t('Proxy mode')}>
        <div className="space-y-3">
          <Segmented<ClashProxyMode> disabled={busy} value={status.proxy_mode}
            options={[{ value: 'rule', label: t('Rule') }, { value: 'global', label: t('Global') }, { value: 'direct', label: t('Direct') }]}
            onChange={v => { if (v !== status.proxy_mode) void update({ proxy_mode: v }, t('Unable to change proxy mode')) }} />
          <p className="text-xs text-ink2">
            {status.proxy_mode === 'rule' && t('Rule: the subscription’s rules decide which traffic is proxied and which goes direct.')}
            {status.proxy_mode === 'global' && t('Global: all traffic from proxy devices uses the node chosen in the GLOBAL group.')}
            {status.proxy_mode === 'direct' && t('Direct: the core stays in the path but sends everything straight out. Useful to rule out node problems.')}
          </p>
          <p className="text-xs text-ink3">{t('The mode applies instantly without restarting the core. Reloading a subscription resets it to this setting.')}</p>
        </div>
      </Card>}

      {status && <Card title={t('Advanced')}>
        <div className="grid gap-4 sm:grid-cols-3">
          <Field label={t('TUN stack')} hint={t('System is fastest; gVisor is more compatible. Applying briefly pauses proxy traffic.')}>
            <Select aria-label={t('TUN stack')} value={status.stack} disabled={busy} onChange={e => void update({ stack: e.target.value as ClashStack }, t('Unable to change TUN stack'))}>
              <option value="system">{t('System')}</option>
              <option value="gvisor">{t('gVisor')}</option>
              <option value="mixed">{t('Mixed (TCP system, UDP gVisor)')}</option>
            </Select>
          </Field>
          <Field label={t('DNS mode')} hint={t('Fake-IP avoids DNS leaks and is recommended. Applying briefly pauses proxy traffic.')}>
            <Select aria-label={t('DNS mode')} value={status.dns_mode} disabled={busy} onChange={e => void update({ dns_mode: e.target.value as ClashDnsMode }, t('Unable to change DNS mode'))}>
              <option value="fake-ip">{t('Fake-IP')}</option>
              <option value="redir-host">{t('Redir-host')}</option>
            </Select>
          </Field>
          <Field label={t('Log level')} hint={t('Info or debug shows each connection in Logs.')}>
            <Select aria-label={t('Log level')} value={status.log_level} disabled={busy} onChange={e => void update({ log_level: e.target.value as ClashLogLevel }, t('Unable to change log level'))}>
              <option value="silent">{t('Silent')}</option>
              <option value="error">{t('Error')}</option>
              <option value="warning">{t('Warning')}</option>
              <option value="info">{t('Info')}</option>
              <option value="debug">{t('Debug')}</option>
            </Select>
          </Field>
        </div>
        <p className="mt-3 text-xs text-ink3">{t('IPv6 internet and IPv6 DNS are blocked for proxy devices so nothing bypasses the core. Router management always stays reachable.')}</p>
      </Card>}
    </div>
  )
}
