import { useState } from 'react'
import { useHome } from '../../app/HomeContext'
import { api } from '../../data/api'
import { formatBandwidthMHz, formatBytes, formatSpeed, formatUptime, modemMode, qualityBg, qualityLabel, qualityText, rsrpQuality, sumBandwidthMHz } from '../../format'
import { t, tm } from '../../i18n'
import { IActivity, IBolt, IDownload, IMonitor, IRadio, IUpload } from '../../icons'
import { Button } from '../../ui/controls'
import { toast } from '../../ui/feedback'
import { Card, Chip, Meter, Row, SignalBars, Skeleton } from '../../ui/primitives'

function PageSkeleton() {
  return (
    <div className="space-y-4">
      <Skeleton className="h-8 w-48" />
      <div className="grid grid-cols-2 gap-3 xl:grid-cols-4">
        <Skeleton className="col-span-2 h-40 xl:col-span-1" />
        <Skeleton className="h-40" />
        <Skeleton className="h-40" />
        <Skeleton className="h-40" />
      </div>
      <div className="grid grid-cols-1 gap-3 lg:grid-cols-3">
        <Skeleton className="h-48" />
        <Skeleton className="h-48" />
        <Skeleton className="h-48" />
      </div>
    </div>
  )
}

export default function HomePage() {
  const { data, error, refresh } = useHome()
  const [toolBusy, setToolBusy] = useState(false)

  async function runQuickAction(fn: () => Promise<unknown>, okMsg: string) {
    setToolBusy(true)
    try {
      await fn()
      toast(okMsg, 'ok')
      refresh()
    } catch (e) {
      toast(tm(e instanceof Error ? e.message : String(e)), 'err')
    } finally {
      setToolBusy(false)
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
  const totalBw = lteBw + nrBw
  const carrierCount = (signal?.lte_carriers.length ?? 0) + (signal?.nr_carriers.length ?? 0)

  return (
    <div className="space-y-4">
      <div>
        <h1 className="text-xl font-bold text-ink">{t('Overview')}</h1>
        <p className="mt-0.5 text-[13px] text-ink2">{signal?.carrier ?? t('Mobile broadband status')}</p>
      </div>

      {error && !data && (
        <Card>
          <p className="text-[13px] text-danger">{tm(error)}</p>
        </Card>
      )}

      {/* Hero stats */}
      <div className="grid grid-cols-2 gap-3 xl:grid-cols-4">
        <Card className="col-span-2 xl:col-span-1">
          <div className="flex h-full flex-col justify-between gap-3">
            <div className="flex items-start justify-between">
              <div>
                <p className="text-[10px] font-semibold uppercase tracking-wider text-ink3">{t('Signal strength')}</p>
                <div className="mt-1.5 flex items-end gap-2">
                  <span className={`tnum text-4xl font-bold leading-none ${qualityText(quality)}`}>
                    {pccRsrp != null ? pccRsrp : '\u2014'}
                  </span>
                  <span className="pb-0.5 text-[11px] font-medium text-ink3">dBm RSRP</span>
                </div>
                <p className={`mt-1 text-[12px] font-semibold ${qualityText(quality)}`}>{qualityLabel(quality)}</p>
              </div>
              <SignalBars bars={signal?.signal_bars} large />
            </div>
            <div className="flex flex-wrap items-center gap-1.5 border-t border-line/8 pt-2.5">
              <span className={`h-2 w-2 rounded-full ${qualityBg(quality)}`} />
              <span className="text-[12px] text-ink2">{t('{n}/5 bars', { n: signal?.signal_bars ?? 0 })}</span>
              {primary?.band && (
                <Chip tone={primary.band.startsWith('n') ? 'nr' : 'lte'}>{primary.band}</Chip>
              )}
              {primary?.pci != null && primary.pci > 0 && (
                <span className="tnum text-[11px] text-ink3">PCI {primary.pci}</span>
              )}
            </div>
          </div>
        </Card>

        <Card>
          <div className="flex h-full flex-col justify-between gap-3">
            <div>
              <div className="flex items-center gap-1.5 text-ink3">
                <IRadio size={14} />
                <p className="text-[10px] font-semibold uppercase tracking-wider">{t('Modem mode')}</p>
              </div>
              <p className="tnum mt-2 text-3xl font-bold text-ink">{mode}</p>
              <p className="mt-1 text-[12px] text-ink2">
                {carrierCount === 1 ? t('1 carrier active') : t('{n} carriers active', { n: carrierCount })}
              </p>
            </div>
            <div className="flex flex-wrap gap-1.5 border-t border-line/8 pt-2.5">
              {nrBw > 0 && <Chip tone="nr">NR {formatBandwidthMHz(nrBw)}</Chip>}
              {lteBw > 0 && <Chip tone="lte">LTE {formatBandwidthMHz(lteBw)}</Chip>}
              {totalBw <= 0 && <span className="text-[11px] text-ink3">{t('No bandwidth reported')}</span>}
            </div>
          </div>
        </Card>

        <Card>
          <div className="flex h-full flex-col justify-between gap-3">
            <div>
              <div className="flex items-center gap-1.5 text-ink3">
                <IActivity size={14} />
                <p className="text-[10px] font-semibold uppercase tracking-wider">{t('Throughput')}</p>
              </div>
              <div className="mt-2 space-y-1.5">
                <div className="flex items-center gap-1.5">
                  <IDownload size={14} className="shrink-0 text-ok" />
                  <span className="tnum text-lg font-bold leading-none text-ink">
                    {speed ? formatSpeed(speed.rx_bps) : '\u2014'}
                  </span>
                </div>
                <div className="flex items-center gap-1.5">
                  <IUpload size={14} className="shrink-0 text-accent" />
                  <span className="tnum text-lg font-bold leading-none text-ink">
                    {speed ? formatSpeed(speed.tx_bps) : '\u2014'}
                  </span>
                </div>
              </div>
            </div>
            <p className="tnum border-t border-line/8 pt-2.5 text-[11px] text-ink3">
              {t('Peak {speed} down', { speed: speed && speed.max_rx_bps > 0 ? formatSpeed(speed.max_rx_bps) : '\u2014' })}
            </p>
          </div>
        </Card>

        <Card>
          <div className="flex h-full flex-col justify-between gap-3">
            <div>
              <div className="flex items-center gap-1.5 text-ink3">
                {battery?.charging ? <IBolt size={14} /> : <IRadio size={14} className="opacity-0" />}
                <p className="text-[10px] font-semibold uppercase tracking-wider">{t('Battery')}</p>
              </div>
              <p className="tnum mt-2 text-3xl font-bold text-ink">
                {battery?.percent != null ? `${battery.percent}%` : '\u2014'}
              </p>
              <p className="mt-1 text-[12px] text-ink2">{battery?.charging ? t('Charging') : t('On battery')}</p>
            </div>
            <p className="tnum border-t border-line/8 pt-2.5 text-[11px] text-ink3">
              {battery?.voltage_mv ? `${(battery.voltage_mv / 1000).toFixed(2)} V` : '\u2014'}
              {battery?.temperature_c != null ? ` · ${battery.temperature_c.toFixed(1)}°C` : ''}
            </p>
          </div>
        </Card>
      </div>

      {/* Pocket OS Quick Toolbox */}
      <Card
        title={t('Pocket OS Quick Toolbox')}
        action={
          <div className="flex items-center gap-1.5">
            <Button
              variant="primary"
              disabled={toolBusy}
              onClick={() => void runQuickAction(() => api.screenOpen(), t('Switched small screen to Pocket OS'))}
            >
              <IMonitor size={14} /> {t('Open Pocket OS')}
            </Button>
            <Button
              variant="outline"
              disabled={toolBusy}
              onClick={() => void runQuickAction(() => api.screenClose(), t('Restored stock ZTE UI on small screen'))}
            >
              {t('Restore stock UI')}
            </Button>
          </div>
        }
      >
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
          <div className="rounded-lg border border-line/8 bg-surface2/40 p-3">
            <p className="text-[11px] font-semibold uppercase tracking-wider text-ink3">{t('Network mode')}</p>
            <div className="mt-2 flex flex-wrap gap-1.5">
              {[
                // The firmware's net_select values (agent/src/cell.rs NETWORK_MODES).
                { id: 'WL_AND_5G', label: t('5G Auto') },
                { id: 'Only_5G', label: '5G SA' },
                { id: 'Only_LTE', label: '4G' },
              ].map((m) => (
                <button
                  key={m.id}
                  disabled={toolBusy}
                  onClick={() => void runQuickAction(() => api.networkModeSet(m.id), t('Network mode updated'))}
                  className={`rounded-md border px-2 py-1 text-[11px] font-semibold transition-colors ${
                    signal?.net_select === m.id
                      ? 'border-accent bg-accent/15 text-accent'
                      : 'border-line/12 bg-surface text-ink2 hover:text-ink'
                  }`}
                >
                  {m.label}
                </button>
              ))}
            </div>
          </div>

          <div className="rounded-lg border border-line/8 bg-surface2/40 p-3">
            <p className="text-[11px] font-semibold uppercase tracking-wider text-ink3">{t('TTL mangle')}</p>
            <div className="mt-2 flex flex-wrap gap-1.5">
              {[
                { ttl: 0, label: t('Off') },
                { ttl: 64, label: 'TTL 64' },
                { ttl: 65, label: 'TTL 65' },
                { ttl: 128, label: 'TTL 128' },
              ].map((item) => (
                <button
                  key={item.ttl}
                  disabled={toolBusy}
                  onClick={() =>
                    void runQuickAction(
                      () => (item.ttl === 0 ? api.ttlClear() : api.ttlSet(item.ttl)),
                      t('TTL setting applied'),
                    )
                  }
                  className="rounded-md border border-line/12 bg-surface px-2 py-1 text-[11px] font-semibold text-ink2 transition-colors hover:border-accent/40 hover:text-ink"
                >
                  {item.label}
                </button>
              ))}
            </div>
          </div>

          <div className="rounded-lg border border-line/8 bg-surface2/40 p-3">
            <p className="text-[11px] font-semibold uppercase tracking-wider text-ink3">{t('Charge limit')}</p>
            <div className="mt-2 flex flex-wrap gap-1.5">
              {[70, 80, 90, 100].map((lim) => (
                <button
                  key={lim}
                  disabled={toolBusy}
                  onClick={() =>
                    void runQuickAction(
                      () =>
                        api.chargeControlSet(
                          lim >= 100
                            ? { charge_limit_enabled: false, charging_stopped: false }
                            : { charge_limit_enabled: true, charge_limit: lim, hysteresis: 10 },
                        ),
                      t('Charge limit updated'),
                    )
                  }
                  className="rounded-md border border-line/12 bg-surface px-2 py-1 text-[11px] font-semibold text-ink2 transition-colors hover:border-accent/40 hover:text-ink"
                >
                  {lim}%
                </button>
              ))}
            </div>
          </div>

          <div className="rounded-lg border border-line/8 bg-surface2/40 p-3">
            <p className="text-[11px] font-semibold uppercase tracking-wider text-ink3">{t('System booster')}</p>
            <div className="mt-2 flex flex-wrap gap-1.5">
              <button
                disabled={toolBusy}
                onClick={() =>
                  void runQuickAction(async () => {
                    const res = await api.killBloat()
                    return res
                  }, t('Killed ZTE bloatware & freed memory'))
                }
                className="rounded-md border border-ok/30 bg-ok/10 px-2.5 py-1 text-[11px] font-semibold text-ok transition-colors hover:bg-ok/15"
              >
                {t('Kill ZTE bloatware')}
              </button>
              <button
                disabled={toolBusy}
                onClick={() => void runQuickAction(() => api.dropCaches(), t('Kernel caches dropped'))}
                className="rounded-md border border-line/12 bg-surface px-2.5 py-1 text-[11px] font-semibold text-ink2 transition-colors hover:border-accent/40 hover:text-ink"
              >
                {t('Drop caches')}
              </button>
            </div>
          </div>
        </div>
      </Card>

      {/* Radio details */}
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

      {/* Details row */}
      <div className="grid grid-cols-1 gap-3 lg:grid-cols-3">
        <Card title={t('Connection')}>
          <Row label={t('Operator')} value={signal?.carrier ?? '\u2014'} />
          <Row label="IPv4" value={wan?.ipv4 ?? '\u2014'} mono />
          <Row label={t('Gateway')} value={wan?.gateway ?? '\u2014'} mono />
          <Row label="IPv6" value={wan6?.ipv6 ?? '\u2014'} mono wrap />
          {wan6?.prefix && <Row label={t('IPv6 prefix')} value={wan6.prefix} mono wrap />}
          {wan?.dns && wan.dns.length > 0 && (
            <Row label="DNS" value={wan.dns.filter((d) => !d.includes(':')).join(', ') || '\u2014'} mono wrap />
          )}
        </Card>

        <Card title={t('Device')}>
          <Row label={t('Model')} value={device?.model ?? '\u2014'} />
          <Row label={t('Firmware')} value={device?.firmware ?? '\u2014'} />
          <Row label={t('Uptime')} value={formatUptime(device?.uptime_secs)} />
          <div className="mt-2 space-y-2 border-t border-line/8 pt-2.5">
            <div>
              <div className="mb-1 flex justify-between text-[11px]">
                <span className="font-medium text-ink2">CPU</span>
                <span className="tnum text-ink2">{cpu ? `${cpu.overall.toFixed(0)}%` : '\u2014'}</span>
              </div>
              <Meter pct={cpu?.overall ?? 0} />
            </div>
            <div>
              <div className="mb-1 flex justify-between text-[11px]">
                <span className="font-medium text-ink2">{t('Memory')}</span>
                <span className="tnum text-ink2">{mem ? `${mem.usage_pct.toFixed(0)}%` : '\u2014'}</span>
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
                  <p className="text-[10px] font-semibold uppercase tracking-wider text-ink3">{label}</p>
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
