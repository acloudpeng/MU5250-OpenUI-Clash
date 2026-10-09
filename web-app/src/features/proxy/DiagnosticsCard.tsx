import { useEffect, useState } from 'react'
import { api } from '../../data/api'
import { ApiError } from '../../data/client'
import { ago, formatDuration } from '../../format'
import { IAlert, ICheck } from '../../icons'
import type { ClashCheck, ClashCheckId, ClashCheckStatus, ClashDiagClient, ClashDiagnostics, ClashVerdict } from '../../types'
import { Button } from '../../ui/controls'
import { toastError } from '../../ui/feedback'
import { Card, Chip, Meter, type ChipTone } from '../../ui/primitives'
import { N_, t, tm } from '../../i18n'

// One-tap Clash diagnosis (agent/src/clash_diag.rs): the agent runs the checks in the
// background and keeps the last report; this card starts a run and polls it.

const POLL_MS = 2000
/** A run takes up to about this long; paces the progress bar. */
const RUN_SECS = 40

const TITLES: Record<ClashCheckId, string> = {
  clock: N_('Clock'),
  core: N_('Core'),
  dns: N_('DNS resolution'),
  nodes: N_('Nodes'),
  fetch: N_('Fetch through the core'),
  devices: N_('Devices'),
  route: N_('Routing'),
  firewall: N_('Firewall'),
}
const STATUS: Record<ClashCheckStatus, { label: string; tone: ChipTone }> = {
  pass: { label: N_('Pass'), tone: 'ok' },
  warn: { label: N_('Warning'), tone: 'warn' },
  fail: { label: N_('Failed'), tone: 'danger' },
  skip: { label: N_('Skipped'), tone: 'default' },
}
type Tone = 'ok' | 'warn' | 'danger'
const VERDICTS: Record<ClashVerdict, { title: string; tone: Tone }> = {
  ok: { title: N_('Devices reach Clash and the node works'), tone: 'ok' },
  nodes: { title: N_('Devices reach Clash, but the node does not work'), tone: 'danger' },
  not_routed: { title: N_('Device traffic does not reach Clash'), tone: 'danger' },
  no_devices: { title: N_('No device uses Clash'), tone: 'warn' },
  core_down: { title: N_('The Clash core is not running'), tone: 'danger' },
  clash_off: { title: N_('Clash is off'), tone: 'warn' },
}
const BOX: Record<Tone, string> = { ok: 'border-ok/25 bg-ok/10', warn: 'border-warn/25 bg-warn/10', danger: 'border-danger/25 bg-danger/10' }
const INK: Record<Tone, string> = { ok: 'text-ok', warn: 'text-warn', danger: 'text-danger' }

/** Agents older than the diagnostics answer 404 "not found". */
const oldAgent = (e: unknown) => e instanceof ApiError && e.status === 404
const OLD_AGENT = N_('This agent cannot run diagnostics; deploy the latest agent.')

function find<K extends ClashCheckId>(checks: ClashCheck[], id: K) {
  return checks.find((c): c is Extract<ClashCheck, { id: K }> => c.id === id)
}

const yesNo = (v?: boolean) => (v ? t('Yes') : t('No'))
const failedOr = (v: number | null | undefined, unit = '') => (v != null ? `${v}${unit}` : t('failed'))

interface Context {
  verdict: ClashVerdict | null
  coreUp: boolean
}

function notChecked(id: ClashCheckId, { verdict, coreUp }: Context) {
  if (verdict === 'clash_off') return t('Not checked while Clash is off')
  if (id === 'route') return t('No connected device to test from')
  return coreUp ? t('Not checked') : t('Not checked while the core is down')
}

/** One line built from a check's data; only the clock keeps data when skipped. */
function summary(c: ClashCheck, ctx: Context): string {
  if (c.status === 'skip' && c.id !== 'clock') return notChecked(c.id, ctx)
  switch (c.id) {
    case 'clock': {
      const { skew_s: skew, ntp } = c.data
      const clock = skew == null ? t('Network time unavailable')
        : Math.abs(skew) < 1 ? t('Device clock matches network time')
        : skew > 0 ? t('Device clock is {d} ahead of network time', { d: formatDuration(skew) })
        : t('Device clock is {d} behind network time', { d: formatDuration(-skew) })
      return ntp == null ? clock : `${clock} · ${ntp ? t('Core NTP: on') : t('Core NTP: off')}`
    }
    case 'core': {
      const { running, supervised, version } = c.data
      if (!running) return supervised ? t('The process runs but its controller does not answer') : t('Not running')
      return [t('Running'), version, supervised === false && t('outside the agent’s supervision')].filter(Boolean).join(' · ')
    }
    case 'dns': {
      const { domestic, foreign } = c.data
      const abroad = foreign ? t('Foreign names resolve') : domestic ? t('Foreign names fail (harmless with Fake-IP)') : t('Foreign names fail')
      return `${domestic ? t('Domestic names resolve') : t('Domestic names fail')} · ${abroad}`
    }
    case 'nodes': {
      const { group, node, https_ms, http_ms, answered = 0, total } = c.data
      const path = group && node && node !== group ? `${group} → ${node}` : node || group
      const answers = total ? t('{answered}/{total} answered', { answered, total }) : ''
      return [path, `HTTPS ${failedOr(https_ms, ' ms')}`, `HTTP ${failedOr(http_ms, ' ms')}`, answers].filter(Boolean).join(' · ')
    }
    case 'fetch':
      return `${t('Domestic {code}', { code: failedOr(c.data.domestic) })} · ${t('Foreign {code}', { code: failedOr(c.data.foreign) })}`
    case 'devices': {
      const { mode, proxied = 0, total = 0 } = c.data
      if (!total) return t('No connected devices found')
      return t('{proxied} of {total} devices use Clash ({mode})', { proxied, total, mode: mode === 'selected' ? t('Selected devices') : t('All devices') })
    }
    case 'route': {
      const { client, tun, dns_tun } = c.data
      const from = client ? t('tested from {client}', { client }) : ''
      return [t('Internet → TUN: {result}', { result: yesNo(tun) }), t('DNS → TUN: {result}', { result: yesNo(dns_tun) }), from].filter(Boolean).join(' · ')
    }
    case 'firewall': {
      const { missing = [], displaced = [], held, rejected, ipa_acceleration: ipa, rp_filter: rp } = c.data
      const facts: string[] = []
      if (missing.length) facts.push(t('Missing hooks: {chains}', { chains: missing.join(', ') }))
      if (displaced.length) facts.push(t('Below firmware rules: {chains}', { chains: displaced.join(', ') }))
      if (held) facts.push(t('Transition guard left in place'))
      if (rejected) facts.push(t('Kill switch rejected {n} packets', { n: rejected }))
      if (!facts.length) facts.push(t('All hooks in place'))
      if (ipa != null) facts.push(ipa ? t('IPA acceleration on') : t('IPA acceleration off'))
      if (rp != null) facts.push(`rp_filter ${rp}`)
      return facts.join(' · ')
    }
  }
}

function guidance(verdict: ClashVerdict, checks: ClashCheck[]): string {
  switch (verdict) {
    case 'ok': return t('If a site still fails, check which rule and node it used under Connections.')
    case 'nodes': return t('Sites that need the proxy fail, while direct sites can still open. Test or switch nodes under Nodes, or update the subscription.')
    case 'not_routed': return t('Sites that need the proxy cannot work until it does. The routing and firewall checks below show where it stops.')
    case 'no_devices': return find(checks, 'devices')?.data.total === 0
      ? t('No connected device was found. Connect one and run the diagnostics again.')
      : t('Device routing keeps every connected device on the normal network. Choose devices under Devices.')
    case 'core_down': return t('Proxy devices have no internet while it is down. Check Logs for the reason.')
    case 'clash_off': return t('Turn Clash on, then run the diagnostics again.')
  }
}

/** What the failing checks point at, for the verdict they produced. */
function hints(verdict: ClashVerdict, checks: ClashCheck[]): string[] {
  const clock = find(checks, 'clock'), route = find(checks, 'route')
  const fetch = find(checks, 'fetch')?.data, devices = find(checks, 'devices')?.data, fw = find(checks, 'firewall')?.data
  const out: string[] = []
  const skew = clock?.data.skew_s
  if (verdict === 'nodes' && clock?.status === 'warn' && skew != null) {
    const d = formatDuration(Math.abs(skew))
    out.push(clock.data.ntp
      ? t('The device clock is off by {d}; VMess and Shadowsocks 2022 nodes reject it until the core’s NTP sync succeeds. If Logs show “Sync time failed”, the core cannot reach its NTP server.', { d })
      : t('The device clock is off by {d}; VMess and Shadowsocks 2022 nodes reject it until the core’s NTP sync succeeds. Core NTP is off: deploy the latest agent.', { d }))
  }
  if (find(checks, 'nodes')?.status === 'warn') {
    out.push(t('HTTP through the node works but HTTPS does not: TLS fails on the router, usually from a wrong clock or missing root certificates (the latest core embeds them).'))
  }
  if (verdict === 'nodes' && fetch?.domestic === null) out.push(t('Domestic sites fail through the core as well, so check the mobile data connection too.'))
  if (verdict === 'not_routed') {
    if (fw?.held) out.push(t('A switch was interrupted and left LAN forwarding blocked; turning Clash off and on again clears it.'))
    if (fw?.missing?.length) out.push(t('Clash firewall hooks are missing ({chains}); turning Clash off and on again reinstalls them.', { chains: fw.missing.join(', ') }))
    if (fw?.displaced?.length) out.push(t('Firmware rules sit above the Clash hooks ({chains}); turning Clash off and on again puts the hooks first.', { chains: fw.displaced.join(', ') }))
    if (route?.status === 'fail') out.push(t('Marked traffic from {client} is not routed into the TUN; check the ip rules below for one that matches first.', { client: route.data.client ?? '—' }))
    if (fw?.rejected) out.push(t('The kill switch rejected {n} packets: marked traffic that was not routed into the TUN.', { n: fw.rejected }))
  }
  // The route and firewall checks are static; connections in the core are the live evidence.
  if ((verdict === 'nodes' || verdict === 'ok') && devices?.proxied && !devices.clients?.some(c => c.proxied && c.active)) {
    out.push(t('The core had no connections from any proxy device. If one was browsing at the time, its traffic is bypassing Clash.'))
  }
  if ((verdict === 'nodes' || verdict === 'not_routed') && fw?.ipa_acceleration) {
    out.push(t('IPA hardware acceleration is on; forwarded traffic may bypass the firewall rules.'))
  }
  return out
}

function Clients({ clients }: { clients: ClashDiagClient[] }) {
  return (
    <ul className="mt-2 space-y-1">
      {clients.map(c => (
        <li key={c.mac || c.ip} className="flex flex-wrap items-center gap-x-2 gap-y-1 rounded-md bg-surface2/60 px-2 py-1.5 text-[12px]">
          <span className="font-mono text-ink">{c.ip}</span>
          {c.name && <span className="min-w-0 truncate text-ink2">{c.name}</span>}
          {c.interface && <span className="font-mono text-[11px] text-ink3">{c.interface}</span>}
          <span className="ml-auto flex gap-1">
            <Chip tone={c.proxied ? 'accent' : 'default'}>{c.proxied ? t('Proxied') : t('Direct')}</Chip>
            {c.active && <span title={t('The core has connections from this device')}><Chip tone="ok">{t('Active')}</Chip></span>}
          </span>
        </li>
      ))}
    </ul>
  )
}

function Verdict({ verdict, checks }: { verdict: ClashVerdict; checks: ClashCheck[] }) {
  const v = VERDICTS[verdict]
  if (!v) return null
  const notes = hints(verdict, checks)
  return (
    <div role="status" className={`rounded-lg border p-3 ${BOX[v.tone]}`}>
      <p className={`flex items-center gap-1.5 text-[13px] font-semibold ${INK[v.tone]}`}>
        {v.tone === 'ok' ? <ICheck size={15} className="shrink-0" /> : <IAlert size={15} className="shrink-0" />}
        {t(v.title)}
      </p>
      <p className="mt-1 text-xs text-ink2">{guidance(verdict, checks)}</p>
      {notes.length > 0 && <ul className="mt-2 list-disc space-y-1 pl-4 text-xs text-ink">{notes.map(n => <li key={n}>{n}</li>)}</ul>}
    </div>
  )
}

export default function DiagnosticsCard() {
  const [report, setReport] = useState<ClashDiagnostics | null>(null)
  const [error, setError] = useState('')
  const [starting, setStarting] = useState(false)
  const [elapsed, setElapsed] = useState(0)
  // Each started run bumps this so the poll below follows it to the end.
  const [round, setRound] = useState(0)
  const running = starting || !!report?.running

  // Loads the last report, then polls while a run is in progress.
  useEffect(() => {
    let alive = true
    let timer: number | undefined
    // A started run keeps polling through a failed read; the first load does not.
    let again = round > 0
    const read = async () => {
      try {
        const r = await api.clashDiagnostics()
        if (!alive) return
        setReport(r); setError('')
        again = r.running
      } catch (e) {
        if (!alive) return
        setError(oldAgent(e) ? t(OLD_AGENT) : e instanceof Error ? e.message : t('Unable to load diagnostics'))
      }
      if (again) timer = window.setTimeout(() => void read(), POLL_MS)
    }
    timer = window.setTimeout(() => void read(), round > 0 ? POLL_MS : 0)
    return () => { alive = false; window.clearTimeout(timer) }
  }, [round])

  // Seconds since this page saw the run start; the device clock may be off.
  useEffect(() => {
    if (!running) return
    const since = Date.now()
    const timer = window.setInterval(() => setElapsed(Math.round((Date.now() - since) / 1000)), 1000)
    return () => window.clearInterval(timer)
  }, [running])

  async function start() {
    setStarting(true); setElapsed(0); setError('')
    try {
      setReport(await api.clashDiagnose())
      setRound(n => n + 1)
    } catch (e) {
      if (oldAgent(e)) setError(t(OLD_AGENT))
      else toastError(e, t('Unable to start diagnostics'))
    } finally {
      setStarting(false)
    }
  }

  const checks = report?.checks ?? []
  const ctx: Context = { verdict: report?.verdict ?? null, coreUp: find(checks, 'core')?.status === 'pass' }
  // finished_at is device time; the clock check measured how far off that is.
  const finished = report?.finished_at != null ? report.finished_at - (find(checks, 'clock')?.data.skew_s ?? 0) : null

  return (
    <Card title={t('Clash diagnostics')} action={
      <Button size="sm" variant="primary" loading={running} onClick={() => void start()}>
        {running ? t('Checking… {n} s', { n: elapsed }) : t('Run diagnostics')}
      </Button>}>
      <div className="space-y-3">
        <p className="text-xs text-ink2">{t('Finds out whether the nodes fail or device traffic never reaches Clash. Changes no settings; takes up to 40 seconds.')}</p>
        {error && <p role="alert" className="rounded-lg bg-danger/10 p-3 text-[13px] text-danger">{tm(error)}</p>}
        {running
          ? <div role="status" className="space-y-2 rounded-lg border border-line/10 bg-surface2/60 p-3">
              <p className="text-[13px] text-ink">{t('Checking the clock, core, DNS, nodes, devices, routing and firewall…')}</p>
              <Meter pct={Math.min(95, (elapsed / RUN_SECS) * 100)} />
            </div>
          : report?.verdict ? <Verdict verdict={report.verdict} checks={checks} />
          : report && <p className="text-sm text-ink3">{t('No results yet.')}</p>}
        {checks.length > 0 && <ul aria-busy={running} className={`divide-y divide-line/10 transition-opacity ${running ? 'opacity-50' : ''}`}>
          {checks.map(c => {
            const status = STATUS[c.status] ?? STATUS.skip
            return (
              <li key={c.id} className="py-2.5 first:pt-0 last:pb-0">
                <div className="flex items-center justify-between gap-2">
                  <span className="text-[13px] font-medium text-ink">{TITLES[c.id] ? t(TITLES[c.id]) : c.id}</span>
                  <Chip tone={status.tone}>{t(status.label)}</Chip>
                </div>
                <p className="mt-0.5 break-words text-xs text-ink2">{summary(c, ctx)}</p>
                {c.id === 'devices' && c.status !== 'skip' && !!c.data.clients?.length && <Clients clients={c.data.clients} />}
                {c.id === 'route' && c.detail && <pre className="mt-2 max-h-40 overflow-auto whitespace-pre rounded-lg bg-surface2/60 p-2.5 font-mono text-[11px] leading-relaxed text-ink2">{c.detail}</pre>}
              </li>
            )
          })}
        </ul>}
        {!running && finished != null && <p className="text-[11px] text-ink3">
          {t('Last run {time}', { time: ago(finished) })}{report?.duration_ms != null ? ` · ${t('took {n} s', { n: Math.round(report.duration_ms / 1000) })}` : ''}
        </p>}
      </div>
    </Card>
  )
}
