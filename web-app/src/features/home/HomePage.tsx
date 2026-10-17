import { useCallback, useEffect, useState, type ReactNode } from 'react'
import { useHome } from '../../app/HomeContext'
import { api } from '../../data/api'
import { formatBandwidthMHz, formatBytes, formatSpeed, formatUptime, modemMode, qualityLabel, qualityText, rsrpQuality, sumBandwidthMHz } from '../../format'
import { t, tm } from '../../i18n'
import { IActivity, IBattery, IBolt, IDownload, IMonitor, IRadio, ISignal, IUpload } from '../../icons'
import type { ChargeControlState, ProxyStatus, TtlStatus } from '../../types'
import { Button, Segmented } from '../../ui/controls'
import { toast } from '../../ui/feedback'
import { Card, Chip, Meter, PageHeader, Row, SignalBars, Skeleton } from '../../ui/primitives'

function PageSkeleton() {
  return (
    <div className="space-y-4">
      <Skeleton className="h-8 w-48" />
      <div className="grid grid-cols-2 gap-3 xl:grid-cols-4">
        <Skeleton className="col-span-2 h-44 xl:col-span-1" />
        <Skeleton className="h-44" />
        <Skeleton className="h-44" />
        <Skeleton className="h-44" />
      </div>
      <Skeleton className="h-56" />
    </div>
  )
}

/** A headline figure: small label with an icon, the value, and a footer line. */
function StatCard({
  icon,
  label,
  children,
  foot,
  className = '',
}: {
  icon: ReactNode
  label: string
  children: ReactNode
  foot?: ReactNode
  className?: string
}) {
  return (
    <Card className={className}>
      <div className="flex h-full flex-col gap-3">
        <div className="flex items-center gap-2 text-ink3">
          <span className="flex h-7 w-7 items-center justify-center rounded-lg bg-surface2 text-ink2">{icon}</span>
          <p className="text-[12px] font-semibold">{label}</p>
        </div>
        <div className="flex-1">{children}</div>
        {foot != null && <div className="border-t border-line/8 pt-2.5 text-[12px] text-ink3">{foot}</div>}
      </div>
    </Card>
  )
}

/** One quick control: its name, what it is now, and the choices. */
function Control({ label, now, children }: { label: string; now?: ReactNode; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-2 rounded-xl bg-surface2/60 p-3">
      <div className="flex items-baseline justify-between gap-2">
        <p className="text-[12px] font-semibold text-ink2">{label}</p>
        {now != null && <p className="truncate text-[12px] font-medium text-ink3">{now}</p>}
      </div>
      <div className="flex flex-wrap gap-1.5">{children}</div>
    </div>
  )
}

// The firmware's net_select values (agent/src/cell.rs NETWORK_MODES).
const NET_MODES = [
  { value: 'WL_AND_5G', label: () => t('5G Auto') },
  { value: 'Only_5G', label: () => '5G SA' },
  { value: 'LTE_AND_5G', label: () => '5G NSA' },
  { value: 'Only_LTE', label: () => '4G' },
]

/** State the quick controls show besides the home poll: read once, and after each change. */
function useControlState() {
  const [ttl, setTtl] = useState<TtlStatus | null>(null)
  const [charge, setCharge] = useState<ChargeControlState | null>(null)
  const [proxy, setProxy] = useState<ProxyStatus | null>(null)
  const [version, setVersion] = useState(0)
  useEffect(() => {
    let live = true
    void Promise.allSettled([api.ttlStatus(), api.chargeControl(), api.proxy()]).then(([a, b, c]) => {
      if (!live) return
      if (a.status === 'fulfilled') setTtl(a.value)
      if (b.status === 'fulfilled') setCharge(b.value)
      if (c.status === 'fulfilled') setProxy(c.value)
    })
    return () => {
      live = false
    }
  }, [version])
  const reload = useCallback(() => setVersion((v) => v + 1), [])
  return { ttl, charge, proxy, reload }
}

export default function HomePage({ onNavigate }: { onNavigate?: (g: 'proxy' | 'devui') => void }) {
  const { data, error, refresh } = useHome()
  const controls = useControlState()
  const [busy, setBusy] = useState(false)

  async function run(fn: () => Promise<unknown>, okMsg: string) {
    setBusy(true)
    try {
      await fn()
      toast(okMsg, 'ok')
      refresh()
      controls.reload()
    } catch (e) {
      toast(tm(e instanceof Error ? e.message : String(e)), 'err')
    } finally {
      setBusy(false)
    }
  }

  if (!data && !error) return <PageSkeleton />

  const signal = data?.signal ?? null
  const battery = data?.battery ?? null
  const speed = data?.speed ?? null
  const device = data?.device ?? null
  const wan = data?.wan ?? null
  const wan6 = data?.wan6 ?? null
  const cpu = data?.cpu ?? null
  const mem = data?.memory ?? null
  const usage = data?.usage ?? null

  const primary = signal?.lte_carriers?.[0] || signal?.nr_carriers?.[0]
  const pccRsrp = primary?.rsrp ?? signal?.rsrp
  const quality = rsrpQuality(pccRsrp)
  const mode = modemMode(signal?.type)
  const lteBw = signal ? sumBandwidthMHz(signal.lte_carriers) : 0
  const nrBw = signal ? sumBandwidthMHz(signal.nr_carriers) : 0
  const carrierCount = (signal?.lte_carriers.length ?? 0) + (signal?.nr_carriers.length ?? 0)

  const ttlValue = controls.ttl ? (controls.ttl.active ? String(controls.ttl.ttl_value ?? '') : '0') : ''
  const limit = controls.charge ? (controls.charge.charge_limit_enabled ? String(controls.charge.charge_limit) : '100') : ''
  const active = controls.proxy?.active
  const proxyName = active === 'wireguard' ? 'WireGuard' : active === 'clash' ? 'Clash' : active === 'off' ? t('Off') : '—'

  return (
    <div className="space-y-4">
      <PageHeader
        title={t('Overview')}
        sub={[signal?.carrier, device?.model].filter(Boolean).join(' · ') || t('Mobile broadband status')}
      />

      {error && !data && (
        <Card>
          <p className="text-[13px] text-danger">{tm(error)}</p>
        </Card>
      )}

      {/* Headline figures */}
      <div className="grid grid-cols-2 gap-3 xl:grid-cols-4">
        <StatCard
          className="col-span-2 xl:col-span-1"
          icon={<ISignal size={15} />}
          label={t('Signal strength')}
          foot={
            <div className="flex flex-wrap items-center gap-1.5">
              <span className={`font-semibold ${qualityText(quality)}`}>{qualityLabel(quality)}</span>
              {primary?.band && <Chip tone={primary.band.startsWith('n') ? 'nr' : 'lte'}>{primary.band}</Chip>}
              {primary?.pci != null && primary.pci > 0 && <span className="tnum">PCI {primary.pci}</span>}
            </div>
          }
        >
          <div className="flex items-end justify-between gap-2">
            <div className="flex items-end gap-1.5">
              <span className={`tnum text-4xl font-bold leading-none ${qualityText(quality)}`}>
                {pccRsrp != null ? pccRsrp : '—'}
              </span>
              <span className="pb-0.5 text-[11px] font-medium text-ink3">dBm</span>
            </div>
            <SignalBars bars={signal?.signal_bars} large />
          </div>
        </StatCard>

        <StatCard
          icon={<IRadio size={15} />}
          label={t('Modem mode')}
          foot={
            <div className="flex flex-wrap gap-1.5">
              {nrBw > 0 && <Chip tone="nr">NR {formatBandwidthMHz(nrBw)}</Chip>}
              {lteBw > 0 && <Chip tone="lte">LTE {formatBandwidthMHz(lteBw)}</Chip>}
              {nrBw + lteBw <= 0 && t('No bandwidth reported')}
            </div>
          }
        >
          <p className="tnum text-3xl font-bold leading-none text-ink">{mode}</p>
          <p className="mt-1.5 text-[12px] text-ink2">
            {carrierCount === 1 ? t('1 carrier active') : t('{n} carriers active', { n: carrierCount })}
          </p>
        </StatCard>

        <StatCard
          icon={<IActivity size={15} />}
          label={t('Throughput')}
          foot={<span className="tnum">{t('Peak {speed} down', { speed: speed && speed.max_rx_bps > 0 ? formatSpeed(speed.max_rx_bps) : '—' })}</span>}
        >
          <div className="space-y-1.5">
            <p className="tnum flex items-center gap-1.5 whitespace-nowrap text-lg font-bold leading-none text-ink sm:text-xl">
              <IDownload size={15} className="shrink-0 text-ok" />
              {speed ? formatSpeed(speed.rx_bps) : '—'}
            </p>
            <p className="tnum flex items-center gap-1.5 whitespace-nowrap text-lg font-bold leading-none text-ink sm:text-xl">
              <IUpload size={15} className="shrink-0 text-accent" />
              {speed ? formatSpeed(speed.tx_bps) : '—'}
            </p>
          </div>
        </StatCard>

        <StatCard
          className="col-span-2 xl:col-span-1"
          icon={battery?.charging ? <IBolt size={15} className="text-ok" /> : <IBattery size={15} />}
          label={t('Battery')}
          foot={
            <span className="tnum">
              {battery?.voltage_mv ? `${(battery.voltage_mv / 1000).toFixed(2)} V` : '—'}
              {battery?.temperature_c != null ? ` · ${battery.temperature_c.toFixed(1)}°C` : ''}
            </span>
          }
        >
          <div className="flex items-end justify-between gap-2">
            <p className="tnum text-3xl font-bold leading-none text-ink">
              {battery?.percent != null ? `${battery.percent}%` : '—'}
            </p>
            <p className={`text-[12px] font-semibold ${battery?.charging ? 'text-ok' : 'text-ink2'}`}>
              {battery?.charging ? t('Charging') : t('On battery')}
            </p>
          </div>
          <Meter
            pct={battery?.percent ?? 0}
            tone={(battery?.percent ?? 100) <= 20 ? 'bg-danger' : 'bg-ok'}
            className="mt-3"
          />
        </StatCard>
      </div>

      {/* Quick controls, each showing what is in effect now */}
      <Card
        title={t('Quick controls')}
        action={
          <div className="flex items-center gap-1.5">
            <Button
              size="sm"
              variant="primary"
              disabled={busy}
              onClick={() => void run(() => api.screenOpen(), t('Switched small screen to DevUI'))}
            >
              <IMonitor size={14} /> {t('Open DevUI')}
            </Button>
            <Button
              size="sm"
              variant="outline"
              disabled={busy}
              onClick={() => void run(() => api.screenClose(), t('Restored stock ZTE UI on small screen'))}
            >
              {t('Restore stock UI')}
            </Button>
          </div>
        }
      >
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-3">
          <Control label={t('Network mode')}>
            <Segmented
              value={signal?.net_select ?? ''}
              disabled={busy}
              options={NET_MODES.map((m) => ({ value: m.value, label: m.label() }))}
              onChange={(v) => v !== signal?.net_select && void run(() => api.networkModeSet(v), t('Network mode updated'))}
            />
          </Control>

          <Control label={t('Proxy')} now={proxyName}>
            <Button size="sm" variant="outline" onClick={() => onNavigate?.('proxy')}>
              {t('Open proxy settings')}
            </Button>
          </Control>

          <Control label={t('TTL mangle')}>
            <Segmented
              value={ttlValue}
              disabled={busy}
              options={[
                { value: '0', label: t('Off') },
                { value: '64', label: '64' },
                { value: '65', label: '65' },
                { value: '128', label: '128' },
              ]}
              onChange={(v) =>
                v !== ttlValue && void run(() => (v === '0' ? api.ttlClear() : api.ttlSet(Number(v))), t('TTL setting applied'))
              }
            />
          </Control>

          <Control label={t('Charge limit')}>
            <Segmented
              value={limit}
              disabled={busy}
              options={['70', '80', '90', '100'].map((v) => ({ value: v, label: `${v}%` }))}
              onChange={(v) =>
                v !== limit && void run(
                  () =>
                    api.chargeControlSet(
                      v === '100'
                        ? { charge_limit_enabled: false, charging_stopped: false }
                        : { charge_limit_enabled: true, charge_limit: Number(v), hysteresis: 10 },
                    ),
                  t('Charge limit updated'),
                )
              }
            />
          </Control>

          <Control label={t('System booster')}>
            <Button size="sm" variant="outline" disabled={busy} onClick={() => void run(() => api.killBloat(), t('Killed ZTE bloatware & freed memory'))}>
              {t('Kill ZTE bloatware')}
            </Button>
            <Button size="sm" variant="outline" disabled={busy} onClick={() => void run(() => api.dropCaches(), t('Kernel caches dropped'))}>
              {t('Drop caches')}
            </Button>
          </Control>

          <Control label={t('Device screen')}>
            <Button size="sm" variant="outline" onClick={() => onNavigate?.('devui')}>
              {t('DevUI settings')}
            </Button>
          </Control>
        </div>
      </Card>

      {/* Carriers */}
      {signal && (signal.lte_carriers.length > 0 || signal.nr_carriers.length > 0) && (
        <Card title={t('Carriers')}>
          <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
            <div>
              <p className="mb-1.5 text-[11px] font-bold uppercase tracking-wider text-accent">LTE</p>
              {signal.lte_carriers.length > 0 ? (
                <div className="flex flex-wrap gap-1.5">
                  {signal.lte_carriers.map((c, i) => (
                    <Chip key={i} tone="lte">
                      {c.label} · {c.band}
                      {c.rsrp != null ? ` · ${c.rsrp} dBm` : ''}
                    </Chip>
                  ))}
                </div>
              ) : (
                <p className="text-[13px] text-ink3">{t('No active LTE carrier')}</p>
              )}
            </div>
            <div>
              <p className="mb-1.5 text-[11px] font-bold uppercase tracking-wider text-violet-600 dark:text-violet-400">
                5G NR
              </p>
              {signal.nr_carriers.length > 0 ? (
                <div className="flex flex-wrap gap-1.5">
                  {signal.nr_carriers.map((c, i) => (
                    <Chip key={i} tone="nr">
                      {c.label} · {c.band}
                      {c.rsrp != null ? ` · ${c.rsrp} dBm` : ''}
                    </Chip>
                  ))}
                </div>
              ) : (
                <p className="text-[13px] text-ink3">{t('No active NR carrier')}</p>
              )}
            </div>
          </div>
        </Card>
      )}

      {/* Details */}
      <div className="grid grid-cols-1 gap-3 lg:grid-cols-3">
        <Card title={t('Connection')}>
          <Row label={t('Operator')} value={signal?.carrier ?? '—'} />
          <Row label="IPv4" value={wan?.ipv4 ?? '—'} mono />
          <Row label={t('Gateway')} value={wan?.gateway ?? '—'} mono />
          <Row label="IPv6" value={wan6?.ipv6 ?? '—'} mono wrap />
          {wan6?.prefix && <Row label={t('IPv6 prefix')} value={wan6.prefix} mono wrap />}
          {wan?.dns && wan.dns.length > 0 && (
            <Row label="DNS" value={wan.dns.filter((d) => !d.includes(':')).join(', ') || '—'} mono wrap />
          )}
        </Card>

        <Card title={t('Device')}>
          <Row label={t('Model')} value={device?.model ?? '—'} />
          <Row label={t('Firmware')} value={device?.firmware ?? '—'} />
          <Row label={t('Uptime')} value={formatUptime(device?.uptime_secs)} />
          <div className="mt-2 space-y-2 border-t border-line/8 pt-2.5">
            <div>
              <div className="mb-1 flex justify-between text-[11px]">
                <span className="font-medium text-ink2">CPU</span>
                <span className="tnum text-ink2">{cpu ? `${cpu.overall.toFixed(0)}%` : '—'}</span>
              </div>
              <Meter pct={cpu?.overall ?? 0} />
            </div>
            <div>
              <div className="mb-1 flex justify-between text-[11px]">
                <span className="font-medium text-ink2">{t('Memory')}</span>
                <span className="tnum text-ink2">{mem ? `${mem.usage_pct.toFixed(0)}%` : '—'}</span>
              </div>
              <Meter pct={mem?.usage_pct ?? 0} tone="bg-warn" />
            </div>
          </div>
        </Card>

        <Card title={t('Data usage')}>
          {usage ? (
            <div className="space-y-2.5">
              {[
                { label: t('Today'), period: usage.day },
                { label: t('This month'), period: usage.month },
                { label: t('Total'), period: usage.total },
              ].map(({ label, period }) => (
                <div key={label}>
                  <p className="text-[11px] font-semibold text-ink3">{label}</p>
                  <div className="tnum mt-0.5 flex gap-3 text-[13px] font-medium">
                    <span className="flex items-center gap-1 text-ok">
                      <IDownload size={12} /> {formatBytes(period.rx_bytes)}
                    </span>
                    <span className="flex items-center gap-1 text-accent">
                      <IUpload size={12} /> {formatBytes(period.tx_bytes)}
                    </span>
                  </div>
                </div>
              ))}
            </div>
          ) : (
            <p className="text-[13px] text-ink3">{t('Not available')}</p>
          )}
        </Card>
      </div>
    </div>
  )
}
