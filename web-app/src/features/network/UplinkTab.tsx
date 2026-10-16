import { useEffect, useState, type ReactNode } from 'react'
import { api } from '../../data/api'
import { ApiError } from '../../data/client'
import { usePoll } from '../../data/poll'
import { formatBytes, formatDuration } from '../../format'
import { IAlert, IChevronDown, ILock, IPencil, IPlus, IRefresh, ITrash, IWifi } from '../../icons'
import type { UplinkNetwork, UplinkNetworkEdit, UplinkSecurity, UplinkSeen, UplinkState, UplinkStatus } from '../../types'
import { Button, Field, Input, Select, Toggle } from '../../ui/controls'
import { confirm, toast, toastError } from '../../ui/feedback'
import { Card, Chip, Empty, Row, SignalBars, Skeleton, Spinner, type ChipTone } from '../../ui/primitives'
import { N_, t, tm } from '../../i18n'

// Wi-Fi relay (agent/src/wifi_uplink.rs): the router joins an upstream Wi-Fi network and
// sends all traffic through it while that network reaches the internet, otherwise over
// the cellular connection. Saves answer at once; the agent applies them in the
// background, so the tab polls the state.

const POLL_MS = 3000
const ACK_KEY = 'u60.uplink.ack'

const STATES: Record<UplinkState, { label: string; tone: ChipTone; text: string }> = {
  off: { label: N_('Off'), tone: 'default', text: 'text-ink2' },
  unavailable: { label: N_('Unavailable'), tone: 'danger', text: 'text-danger' },
  connecting: { label: N_('Connecting…'), tone: 'accent', text: 'text-accent' },
  obtaining_ip: { label: N_('Getting an IP address…'), tone: 'accent', text: 'text-accent' },
  checking: { label: N_('Checking internet access…'), tone: 'accent', text: 'text-accent' },
  online: { label: N_('Online — traffic on Wi-Fi'), tone: 'ok', text: 'text-ok' },
  no_internet: { label: N_('No internet — traffic on cellular'), tone: 'warn', text: 'text-warn' },
  needs_login: { label: N_('Needs a web login — traffic on cellular'), tone: 'warn', text: 'text-warn' },
  logging_in: { label: N_('Login mode — traffic on Wi-Fi'), tone: 'accent', text: 'text-accent' },
}
const SECURITY: Record<UplinkSecurity, string> = {
  open: N_('Open'),
  wpa2: 'WPA2', // i18n-ignore
  wpa3: 'WPA3', // i18n-ignore
  wpa2_wpa3: 'WPA2/WPA3', // i18n-ignore
}
const SECURITY_ORDER: UplinkSecurity[] = ['wpa2_wpa3', 'wpa2', 'wpa3', 'open']

/** Agents older than the relay answer 404. */
const OLD_AGENT = N_('This agent has no Wi-Fi relay support; deploy the latest agent.')

// Validation as the agent does it; the messages are the agent's, translated by tm().
const SSID_RULE = 'Network name must be 1–32 bytes without control characters' // i18n-ignore
const PASSWORD_RULE = 'Wi-Fi password must be 8–63 printable ASCII characters' // i18n-ignore
const WPA3_HEX_RULE = 'A WPA3 password cannot be a 64-digit key' // i18n-ignore

const isControl = (c: number) => c < 0x20 || (c >= 0x7f && c <= 0x9f)

function ssidProblem(ssid: string): string | null {
  const bytes = new TextEncoder().encode(ssid).length
  if (bytes < 1 || bytes > 32 || Array.from(ssid).some((ch) => isControl(ch.codePointAt(0) ?? 0))) return tm(SSID_RULE)
  return null
}

function passwordProblem(security: UplinkSecurity, password: string): string | null {
  if (security === 'open') return null
  const hex = /^[0-9a-fA-F]{64}$/.test(password)
  if (hex) return security === 'wpa3' ? tm(WPA3_HEX_RULE) : null
  const printable = Array.from(password).every((ch) => {
    const c = ch.codePointAt(0) ?? 0
    return c >= 0x20 && c <= 0x7e
  })
  return password.length >= 8 && password.length <= 63 && printable ? null : tm(PASSWORD_RULE)
}

/** Signal bars (1–5) for a Wi-Fi RSSI in dBm. */
function bars(dbm: number | null | undefined) {
  if (dbm == null) return 0
  return dbm >= -55 ? 5 : dbm >= -65 ? 4 : dbm >= -72 ? 3 : dbm >= -80 ? 2 : 1
}

function bandText(band?: string | null, channel?: number | null) {
  const parts = [band ? `${band} GHz` : '', channel != null ? t('Channel {n}', { n: channel }) : ''].filter(Boolean)
  return parts.join(' · ') || '—'
}

function acknowledged() {
  try {
    return localStorage.getItem(ACK_KEY) === '1'
  } catch {
    return false
  }
}

function acknowledge() {
  try {
    localStorage.setItem(ACK_KEY, '1')
  } catch {
    // Private mode: the notice shows again next time.
  }
}

/** What turning the relay on changes, asked once per browser. */
async function confirmEnable() {
  if (acknowledged()) return true
  const ok = await confirm({
    title: t('Turn on the Wi-Fi relay?'),
    body: t(
      'The router joins the upstream Wi-Fi with the same radio as its hotspot. The hotspot may move to the upstream network’s channel, so connected devices can drop for a few seconds. While the upstream Wi-Fi reaches the internet, all traffic goes through it, including the router’s own; otherwise traffic stays on the cellular connection. IPv6 for LAN devices is held while traffic is on Wi-Fi. Networks with a web login page (captive portals) are not supported yet, and the upstream network must use a different subnet than this router’s LAN.',
    ),
    confirmLabel: t('Turn on'),
  })
  if (ok) acknowledge()
  return ok
}

const editsOf = (list: UplinkNetwork[]): UplinkNetworkEdit[] =>
  list.map((n) => ({ id: n.id, ssid: n.ssid, security: n.security, hidden: n.hidden }))

// ── Dialog ────────────────────────────────────────────────────────────────────

function Modal({ title, onClose, children }: { title: string; onClose: () => void; children: ReactNode }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])
  return (
    <div className="fixed inset-0 z-50 flex items-end justify-center bg-black/45 p-4 sm:items-center" onClick={onClose}>
      <div
        className="w-full max-w-sm rounded-xl border border-line/10 bg-surface p-4 shadow-xl"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-label={title}
      >
        <h3 className="text-sm font-semibold text-ink">{title}</h3>
        <div className="mt-3">{children}</div>
      </div>
    </div>
  )
}

/** Join, add or edit one network. `saved` is the saved entry being edited, if any. */
interface Draft {
  ssid: string
  security: UplinkSecurity
  hidden: boolean
  /** Name and security come from a scan: shown, not edited. */
  scanned: boolean
  saved?: UplinkNetwork
}

function NetworkDialog({
  draft,
  busy,
  canAdd,
  onClose,
  onSave,
}: {
  draft: Draft
  busy: boolean
  canAdd: boolean
  onClose: () => void
  onSave: (edit: UplinkNetworkEdit, connect: boolean) => void
}) {
  const [ssid, setSsid] = useState(draft.ssid)
  const [security, setSecurity] = useState<UplinkSecurity>(draft.security)
  const [password, setPassword] = useState('')
  const [hidden, setHidden] = useState(draft.hidden)
  const keeps = !!draft.saved?.has_password && draft.saved.security !== 'open' && security !== 'open' && password === ''
  const ssidBad = draft.scanned || draft.saved ? null : ssidProblem(ssid)
  const passwordBad = keeps ? null : passwordProblem(security, password)
  const showSsidError = ssid !== '' && !!ssidBad
  const showPasswordError = password !== '' && !!passwordBad
  const ready = !ssidBad && !passwordBad && (canAdd || !!draft.saved)
  const title = draft.saved ? t('Edit {ssid}', { ssid: draft.saved.ssid }) : draft.scanned ? t('Join {ssid}', { ssid: draft.ssid }) : t('Add a hidden network')

  function submit(connect: boolean) {
    onSave(
      {
        ...(draft.saved ? { id: draft.saved.id } : {}),
        ssid: draft.saved ? draft.saved.ssid : ssid,
        security,
        ...(security !== 'open' && !keeps ? { password } : {}),
        hidden,
      },
      connect,
    )
  }

  return (
    <Modal title={title} onClose={onClose}>
      <form
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault()
          if (ready) submit(true)
        }}
      >
        {!draft.scanned && !draft.saved && (
          <Field label={t('Network name (SSID)')}>
            <Input aria-label={t('Network name (SSID)')} aria-invalid={showSsidError} autoComplete="off" spellCheck={false}
              value={ssid} onChange={(e) => setSsid(e.target.value)} autoFocus />
          </Field>
        )}
        {showSsidError && <p className="text-[11px] text-danger">{ssidBad}</p>}
        <Field label={t('Security')}>
          <Select aria-label={t('Security')} value={security} disabled={draft.scanned}
            onChange={(e) => setSecurity(e.target.value as UplinkSecurity)}>
            {SECURITY_ORDER.map((s) => (
              <option key={s} value={s}>{t(SECURITY[s])}</option>
            ))}
          </Select>
        </Field>
        {security !== 'open' && (
          <Field label={t('Password')} hint={draft.saved?.has_password ? t('Leave empty to keep the saved password.') : undefined}>
            <Input aria-label={t('Password')} aria-invalid={showPasswordError} type="password" autoComplete="new-password"
              spellCheck={false} value={password} onChange={(e) => setPassword(e.target.value)} autoFocus={draft.scanned} />
          </Field>
        )}
        {showPasswordError && <p className="text-[11px] text-danger">{passwordBad}</p>}
        {!draft.scanned && (
          <label className="flex items-start gap-2 text-[13px] text-ink2">
            <input type="checkbox" className="mt-0.5" checked={hidden} onChange={(e) => setHidden(e.target.checked)} />
            <span>{t('Hidden network (it does not broadcast its name)')}</span>
          </label>
        )}
        {!canAdd && !draft.saved && <p className="text-[11px] text-danger">{t('The list is full; delete a saved network first.')}</p>}
        <div className="flex flex-wrap justify-end gap-2 pt-1">
          <Button type="button" variant="ghost" onClick={onClose}>{t('Cancel')}</Button>
          <Button type="button" variant="subtle" disabled={!ready} loading={busy} onClick={() => submit(false)}>{t('Save')}</Button>
          <Button type="submit" variant="primary" disabled={!ready} loading={busy}>{t('Save and connect')}</Button>
        </div>
      </form>
    </Modal>
  )
}

// ── Status ────────────────────────────────────────────────────────────────────

function minutes(secs: number) {
  return `${Math.floor(secs / 60)}:${String(secs % 60).padStart(2, '0')}`
}

/** Login mode for networks with a captive portal: traffic goes over the Wi-Fi for ten
 *  minutes so the login page opens on a phone or computer on this hotspot. */
function LoginPanel({ status, busy, onLogin }: { status: UplinkStatus; busy: boolean; onLogin: (on: boolean) => void }) {
  const active = status.state === 'logging_in'
  const offered = status.enabled && !!status.ip && (status.state === 'needs_login' || status.state === 'no_internet')
  if (!active && !offered) return null
  const url = status.portal_url
  return (
    <div className="space-y-2 rounded-lg border border-line/10 bg-surface2 p-3 text-[13px]">
      {active ? (
        <>
          <p className="text-ink">
            {t('Sign in on the login page from a phone or computer on this hotspot. Login mode ends in {time}, or as soon as the internet works.', {
              time: minutes(status.login_remaining ?? 0),
            })}
          </p>
          <div className="flex flex-wrap gap-2">
            {url && (
              <a className="inline-flex h-8 items-center rounded-lg bg-accent px-3 text-xs font-semibold text-white hover:brightness-110" href={url} target="_blank" rel="noreferrer noopener">
                {t('Open the login page')}
              </a>
            )}
            <Button size="sm" variant="ghost" disabled={busy} onClick={() => onLogin(false)}>{t('End login mode')}</Button>
          </div>
          {url && <p className="break-all text-xs text-ink3">{url}</p>}
        </>
      ) : (
        <>
          <p className="text-ink">
            {status.state === 'needs_login'
              ? t('This network wants you to sign in on a web page first (common in hotels and airports).')
              : t('If this network needs a web login, login mode lets you reach its page.')}
          </p>
          <Button size="sm" variant="primary" disabled={busy} onClick={() => onLogin(true)}>{t('Start web login (10 min)')}</Button>
          <p className="text-xs text-ink3">
            {t('Signing in once covers every device on this hotspot. If the page does not open with a proxy on, turn the proxy off while signing in.')}
          </p>
        </>
      )}
    </div>
  )
}

function StatusCard({ status, busy, onToggle, onLogin }: { status: UplinkStatus; busy: boolean; onToggle: (on: boolean) => void; onLogin: (on: boolean) => void }) {
  const info = STATES[status.state] ?? STATES.unavailable
  const joined = ['checking', 'online', 'no_internet', 'obtaining_ip', 'needs_login', 'logging_in'].includes(status.state)
  const noNetworks = status.networks.length === 0
  return (
    <Card
      title={t('Wi-Fi relay')}
      action={
        <Toggle
          checked={status.enabled}
          disabled={busy || (!status.enabled && noNetworks)}
          onChange={onToggle}
          label={t('Wi-Fi relay')}
        />
      }
    >
      <div className="space-y-3">
        <div className="flex items-start gap-3">
          <div className={`mt-0.5 shrink-0 ${info.text}`}>
            {status.state === 'connecting' || status.state === 'obtaining_ip' || status.state === 'checking'
              ? <Spinner size={22} />
              : <IWifi size={22} />}
          </div>
          <div className="min-w-0">
            <p className={`text-lg font-semibold leading-snug ${info.text}`} role="status">{t(info.label)}</p>
            <p className="mt-0.5 text-xs text-ink2">
              {status.state === 'online' && status.ssid
                ? t('All traffic goes through {ssid}.', { ssid: status.ssid })
                : status.state === 'logging_in'
                  ? t('Traffic goes over this Wi-Fi so its login page can open.')
                  : status.enabled
                  ? t('Traffic stays on the cellular connection until the upstream Wi-Fi reaches the internet.')
                  : t('Join an upstream Wi-Fi network (home, hotel, office) and use it instead of cellular data.')}
            </p>
          </div>
        </div>
        {status.reason && (
          <div role="alert" className="flex items-start gap-2 rounded-lg border border-warn/25 bg-warn/10 p-2.5 text-[13px] text-ink">
            <IAlert size={15} className="mt-0.5 shrink-0 text-warn" />
            <span className="min-w-0 break-words">{tm(status.reason)}</span>
          </div>
        )}
        <LoginPanel status={status} busy={busy} onLogin={onLogin} />
        {!status.tools.wpa_supplicant && (
          <p className="text-xs text-danger">{t('This device has no wpa_supplicant, so the relay cannot join a network.')}</p>
        )}
        {!status.enabled && noNetworks && <p className="text-xs text-ink3">{tm('Add an upstream Wi-Fi network first')}</p>}
        {joined && (
          <div className="grid gap-x-6 sm:grid-cols-2">
            <div>
              <Row label={t('Network')} value={status.ssid || '—'} />
              <Row label={t('Band')} value={bandText(status.band, status.channel)} />
              <Row
                label={t('Signal')}
                value={
                  status.signal != null ? (
                    <span className="inline-flex items-center gap-2"><SignalBars bars={bars(status.signal)} /><span className="tnum">{status.signal} dBm</span></span>
                  ) : '—'
                }
              />
              {status.bssid && <Row label="BSSID" value={status.bssid} mono />}
            </div>
            <div>
              <Row label={t('IP address')} value={status.ip ? `${status.ip}${status.prefix != null ? `/${status.prefix}` : ''}` : '—'} mono />
              <Row label={t('Gateway')} value={status.gateway || '—'} mono />
              <Row label="DNS" value={status.dns.length ? status.dns.join(', ') : '—'} mono wrap />
              <Row label={t('Traffic (received / sent)')} value={`${formatBytes(status.rx_bytes)} / ${formatBytes(status.tx_bytes)}`} />
              {status.online && status.online_for != null && <Row label={t('Online for')} value={formatDuration(status.online_for)} />}
            </div>
          </div>
        )}
      </div>
    </Card>
  )
}

// ── Saved networks ────────────────────────────────────────────────────────────

function SavedCard({
  status,
  busy,
  onMove,
  onEdit,
  onDelete,
  onAdd,
}: {
  status: UplinkStatus
  busy: boolean
  onMove: (index: number, by: -1 | 1) => void
  onEdit: (n: UplinkNetwork) => void
  onDelete: (n: UplinkNetwork) => void
  onAdd: () => void
}) {
  const list = status.networks
  const full = list.length >= status.max_networks
  return (
    <Card
      title={t('Saved networks ({count}/{max})', { count: list.length, max: status.max_networks })}
      action={
        <Button size="sm" variant="ghost" disabled={busy || full} onClick={onAdd}>
          <IPlus size={14} />{t('Hidden network')}
        </Button>
      }
    >
      {list.length === 0 ? (
        <Empty icon={<IWifi size={22} />} title={t('No saved networks')} body={t('Scan for networks below, or add a hidden one.')} />
      ) : (
        <div className="space-y-2">
          <p className="text-xs text-ink3">{t('The first network in range is joined; reorder to set the preference.')}</p>
          <ul className="divide-y divide-line/8">
            {list.map((n, i) => {
              const current = status.network_id === n.id && status.state !== 'off' && status.state !== 'connecting'
              return (
                <li key={n.id} className="flex items-center gap-2 py-2">
                  <span className="tnum w-5 shrink-0 text-center text-xs text-ink3">{i + 1}</span>
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-[13px] font-medium text-ink">{n.ssid}</p>
                    <div className="mt-0.5 flex flex-wrap gap-1">
                      <Chip>{t(SECURITY[n.security])}</Chip>
                      {n.hidden && <Chip>{t('Hidden')}</Chip>}
                      {n.security !== 'open' && !n.has_password && <Chip tone="danger">{t('No password')}</Chip>}
                      {current && <Chip tone={status.online ? 'ok' : 'accent'}>{t('In use')}</Chip>}
                    </div>
                  </div>
                  <div className="flex shrink-0 items-center gap-0.5">
                    <Button size="sm" variant="ghost" className="w-8 px-0" aria-label={t('Move up')} disabled={busy || i === 0} onClick={() => onMove(i, -1)}>
                      <IChevronDown size={15} className="shrink-0 rotate-180" />
                    </Button>
                    <Button size="sm" variant="ghost" className="w-8 px-0" aria-label={t('Move down')} disabled={busy || i === list.length - 1} onClick={() => onMove(i, 1)}>
                      <IChevronDown size={15} className="shrink-0" />
                    </Button>
                    <Button size="sm" variant="ghost" className="w-8 px-0" aria-label={t('Edit')} disabled={busy} onClick={() => onEdit(n)}>
                      <IPencil size={14} className="shrink-0" />
                    </Button>
                    <Button size="sm" variant="ghost" className="w-8 px-0 text-danger" aria-label={t('Delete')} disabled={busy} onClick={() => onDelete(n)}>
                      <ITrash size={14} className="shrink-0" />
                    </Button>
                  </div>
                </li>
              )
            })}
          </ul>
        </div>
      )}
    </Card>
  )
}

// ── Scan ──────────────────────────────────────────────────────────────────────

function ScanCard({
  saved,
  busy,
  onPick,
}: {
  saved: UplinkNetwork[]
  busy: boolean
  onPick: (s: UplinkSeen) => void
}) {
  const [scanning, setScanning] = useState(false)
  const [results, setResults] = useState<UplinkSeen[] | null>(null)

  async function scan() {
    setScanning(true)
    try {
      const found = await api.uplinkScan()
      setResults([...found].sort((a, b) => b.signal - a.signal))
    } catch (e) {
      toastError(e, t('Wi-Fi scan failed'))
    } finally {
      setScanning(false)
    }
  }

  return (
    <Card
      title={t('Nearby networks')}
      action={
        <Button size="sm" variant="ghost" loading={scanning} onClick={() => void scan()}>
          {!scanning && <IRefresh size={14} />}{scanning ? t('Scanning…') : t('Scan')}
        </Button>
      }
    >
      {scanning && !results ? (
        <div className="space-y-2" aria-busy={true}>
          <p className="text-xs text-ink3">{t('Scanning takes up to 10 seconds; the hotspot stays on.')}</p>
          <Skeleton className="h-10" />
          <Skeleton className="h-10" />
          <Skeleton className="h-10" />
        </div>
      ) : !results ? (
        <p className="text-xs text-ink3">{t('Scan to list the Wi-Fi networks around the router.')}</p>
      ) : results.length === 0 ? (
        <Empty icon={<IWifi size={22} />} title={t('No Wi-Fi networks found')} />
      ) : (
        <ul className={`divide-y divide-line/8 ${scanning ? 'opacity-60' : ''}`}>
          {results.map((s) => {
            const known = saved.some((n) => n.ssid === s.ssid)
            const supported = s.supported && s.security != null
            return (
              <li key={s.bssid || s.ssid}>
                <button
                  type="button"
                  disabled={!supported || busy}
                  onClick={() => onPick(s)}
                  className="flex w-full items-center gap-3 py-2 text-left transition-colors enabled:hover:bg-surface2/60 disabled:opacity-55"
                >
                  <SignalBars bars={bars(s.signal)} />
                  <div className="min-w-0 flex-1">
                    <p className="flex min-w-0 items-center gap-1.5 text-[13px] font-medium text-ink">
                      <span className="truncate">{s.ssid}</span>
                      {s.security !== 'open' && <ILock size={12} className="shrink-0 text-ink3" />}
                    </p>
                    <p className="mt-0.5 text-[11px] text-ink3">
                      {bandText(s.band, s.channel)} · <span className="tnum">{s.signal} dBm</span> ·{' '}
                      {s.security ? t(SECURITY[s.security]) : t('Not supported (WEP or enterprise)')}
                    </p>
                  </div>
                  {known && <Chip tone="accent">{t('Saved')}</Chip>}
                </button>
              </li>
            )
          })}
        </ul>
      )}
    </Card>
  )
}

// ── Tab ───────────────────────────────────────────────────────────────────────

export default function UplinkTab() {
  const { data, error, refresh, mutate } = usePoll('uplink', api.uplink, POLL_MS)
  const [busy, setBusy] = useState(false)
  const [draft, setDraft] = useState<Draft | null>(null)

  async function save(networks: UplinkNetworkEdit[] | undefined, enabled: boolean | undefined, done?: string) {
    if (!data) return false
    setBusy(true)
    try {
      const next = await api.uplinkSave({
        expected_revision: data.revision,
        ...(networks ? { networks } : {}),
        ...(enabled !== undefined ? { enabled } : {}),
      })
      mutate(next)
      if (done) toast(done)
      return true
    } catch (e) {
      toastError(e)
      if (e instanceof ApiError && e.status === 409) refresh()
      return false
    } finally {
      setBusy(false)
    }
  }

  if (!data) {
    if (error) {
      const old = /not found/i.test(error)
      return (
        <Card>
          <div role="alert" className="flex items-start gap-2 text-[13px] text-danger">
            <IAlert size={15} className="mt-0.5 shrink-0" />
            <span>{old ? t(OLD_AGENT) : tm(error)}</span>
          </div>
        </Card>
      )
    }
    return (
      <div className="space-y-4">
        <Skeleton className="h-36" />
        <Skeleton className="h-28" />
      </div>
    )
  }

  const status = data
  const canAdd = status.networks.length < status.max_networks

  async function login(on: boolean) {
    setBusy(true)
    try {
      mutate(await api.uplinkLogin(on))
      toast(on ? t('Login mode on for 10 minutes') : t('Login mode ended'))
    } catch (e) {
      toastError(e)
    } finally {
      setBusy(false)
    }
  }

  async function toggle(on: boolean) {
    if (on && !(await confirmEnable())) return
    await save(undefined, on, on ? t('Wi-Fi relay turned on') : t('Wi-Fi relay turned off — traffic is back on cellular'))
  }

  function move(index: number, by: -1 | 1) {
    const list = editsOf(status.networks)
    const [item] = list.splice(index, 1)
    list.splice(index + by, 0, item)
    void save(list, undefined)
  }

  async function remove(n: UplinkNetwork) {
    const ok = await confirm({
      title: t('Delete {ssid}?', { ssid: n.ssid }),
      body: status.network_id === n.id && status.enabled ? t('The relay is using this network; it will look for the next saved one.') : undefined,
      confirmLabel: t('Delete'),
      danger: true,
    })
    if (!ok) return
    const rest = status.networks.filter((x) => x.id !== n.id)
    // The agent refuses on with no networks; turning off with the last one goes in the same save.
    await save(editsOf(rest), rest.length === 0 && status.enabled ? false : undefined, t('Deleted {ssid}', { ssid: n.ssid }))
  }

  function pick(s: UplinkSeen) {
    const saved = status.networks.find((n) => n.ssid === s.ssid)
    if (saved) setDraft({ ssid: saved.ssid, security: saved.security, hidden: saved.hidden, scanned: false, saved })
    else setDraft({ ssid: s.ssid, security: s.security ?? 'wpa2', hidden: false, scanned: true })
  }

  async function saveNetwork(edit: UplinkNetworkEdit, connect: boolean) {
    const others = editsOf(status.networks).filter((n) => n.id !== edit.id)
    const index = status.networks.findIndex((n) => n.id === edit.id)
    const networks = connect ? [edit, ...others] : index >= 0 ? editsOf(status.networks).map((n) => (n.id === edit.id ? edit : n)) : [...others, edit]
    const enable = connect && !status.enabled
    if (enable && !(await confirmEnable())) return
    const ok = await save(networks, enable ? true : undefined, connect ? t('Saved — connecting to {ssid}', { ssid: edit.ssid }) : t('Saved'))
    if (ok) setDraft(null)
  }

  return (
    <div className="space-y-4">
      <StatusCard status={status} busy={busy} onToggle={(on) => void toggle(on)} onLogin={(on) => void login(on)} />
      <SavedCard
        status={status}
        busy={busy}
        onMove={move}
        onEdit={(n) => setDraft({ ssid: n.ssid, security: n.security, hidden: n.hidden, scanned: false, saved: n })}
        onDelete={(n) => void remove(n)}
        onAdd={() => setDraft({ ssid: '', security: 'wpa2_wpa3', hidden: true, scanned: false })}
      />
      <ScanCard saved={status.networks} busy={busy} onPick={pick} />
      {draft && (
        <NetworkDialog
          key={`${draft.saved?.id ?? 'new'}:${draft.ssid}`}
          draft={draft}
          busy={busy}
          canAdd={canAdd}
          onClose={() => setDraft(null)}
          onSave={(edit, connect) => void saveNetwork(edit, connect)}
        />
      )}
    </div>
  )
}
