import { useCallback, useEffect, useRef, useState, type ClipboardEvent, type ReactNode } from 'react'
import { api } from '../../data/api'
import { ApiError } from '../../data/client'
import { formatBytes } from '../../format'
import { IAlert, ICheck, IPencil, IRefresh, ISim, ITrash, IUpload, IX } from '../../icons'
import type { EsimAction, EsimJob, EsimJobKind, EsimNotification, EsimProfile, EsimRoute, EsimStatus } from '../../types'
import { Button, Field, Input, Select } from '../../ui/controls'
import { confirm, toast, toastError } from '../../ui/feedback'
import { Card, Chip, Empty, Spinner } from '../../ui/primitives'
import { N_, t, tm } from '../../i18n'
import { parseActivation, readQr, validMatchingId, validSmdp } from './qr'

// eSIM on a removable eUICC in the SIM tray (agent/src/esim.rs), as EasyEUICC offers it on
// Android. Each action starts a job on the agent and answers at once; the tab follows the
// job until it ends. Device times may be hours off, so only durations are shown.

const POLL_MS = 1500
const RETRY_MS = 5000

const STEPS: Record<string, string> = {
  reading: N_('Reading the card'),
  es10c_enable_profile: N_('Enabling the profile on the card'),
  es10c_disable_profile: N_('Disabling the profile on the card'),
  radio_offline: N_('Turning the radio off'),
  sim_power_cycle: N_('Restarting the SIM'),
  radio_online: N_('Turning the radio back on'),
  sim_reading: N_('Waiting for the modem to read the card'),
  firmware: N_('Waiting for the firmware to use the profile'),
  es10c_delete_profile: N_('Deleting the profile'),
  es10c_set_nickname: N_('Saving the nickname'),
  es10a_get_euicc_configured_addresses: N_('Reading the default SM-DP+ address'),
  es10b_get_euicc_challenge_and_info: N_('Preparing the card'),
  es9p_initiate_authentication: N_('Contacting the SM-DP+ server'),
  es10b_authenticate_server: N_('Checking the server'),
  es9p_authenticate_client: N_('Authenticating the card'),
  es8p_meatadata_parse: N_('Reading the profile details'),
  es10b_prepare_download: N_('Preparing the download'),
  es9p_get_bound_profile_package: N_('Downloading the profile'),
  es10b_load_bound_profile_package: N_('Installing the profile on the card'),
  es10b_cancel_session: N_('Cancelling the download'),
  es9p_cancel_session: N_('Cancelling the download'),
  es10b_list_notification: N_('Reading notifications'),
  es10b_retrieve_notifications_list: N_('Reading notifications'),
  es9p_handle_notification: N_('Sending notifications'),
  es10b_remove_notification_from_list: N_('Removing notifications'),
}
const RUNNING: Record<EsimJobKind, string> = {
  scan: N_('Reading the card…'),
  enable: N_('Switching profiles…'),
  disable: N_('Disabling the profile…'),
  nickname: N_('Saving the nickname…'),
  delete: N_('Deleting the profile…'),
  download: N_('Downloading the profile…'),
  notifications: N_('Handling notifications…'),
}
const FAILED: Record<EsimJobKind, string> = {
  scan: N_('Reading the card failed'),
  enable: N_('Switching failed'),
  disable: N_('Disabling failed'),
  nickname: N_('Renaming failed'),
  delete: N_('Deleting failed'),
  download: N_('Download failed'),
  notifications: N_('Notifications failed'),
}
const OPERATIONS: Record<string, string> = {
  install: N_('Install'),
  enable: N_('Enable'),
  disable: N_('Disable'),
  delete: N_('Delete'),
}
const CLASSES: Record<string, string> = {
  test: N_('Test'),
  provisioning: N_('Provisioning'),
}
const ROUTE_HINTS: Record<EsimRoute, string> = {
  auto: N_('Through the Clash core while it runs, else directly.'),
  proxy: N_('Through the running Clash core and its selected node.'),
  direct: N_('Through the router’s mobile connection, without Clash.'),
}

const SMDP_EXAMPLE = 'rsp.example.com' // i18n-ignore
const CODE_EXAMPLE = 'LPA:1$rsp.example.com$ABCD-1234' // i18n-ignore

/** Agents older than eSIM support answer 404 "not found". */
const OLD_AGENT = N_('This agent has no eSIM support; deploy the latest agent.')

const jobId = (j: EsimJob) => `${j.id}:${j.started_at}`
const label = (p: EsimProfile) => p.nickname || p.name || p.provider || p.iccid
const shortIccid = (iccid: string) => (iccid.length > 8 ? `…${iccid.slice(-6)}` : iccid)
/** As the agent compares them: uppercase, without F padding. */
const normalIccid = (iccid: string) => iccid.trim().toUpperCase().replace(/F+$/, '')

/** Names of the profiles this page has shown, so a deleted one is still named. */
const knownNames = new Map<string, string>()

function doneMessage(job: EsimJob, status: EsimStatus): string | null {
  const r = job.result ?? {}
  const profile = status.view?.profiles.find((p) => p.iccid === r.iccid)
  const name = profile ? label(profile) : knownNames.get(String(r.iccid ?? '')) ?? String(r.iccid ?? '')
  switch (job.kind) {
    case 'scan': return null
    case 'enable': return r.firmware_ready ? t('Switched to {name}.', { name })
      : t('The card switched to {name}, but the firmware still uses the previous profile. Restart the device to finish.', { name })
    case 'disable': return t('{name} disabled. The router has no mobile service until you enable a profile.', { name })
    case 'nickname': return t('Nickname saved.')
    case 'delete': return t('{name} deleted.', { name })
    case 'download': return t('Downloaded {name}. It stays disabled until you enable it.', { name })
    case 'notifications': return t('Notifications updated.')
  }
}

/** A delete or download leaves notifications for the SM-DP+; say when sending them failed. */
function notifyNote(job: EsimJob) {
  const n = job.result?.notifications as { sent?: number; error?: string } | undefined
  return n?.error ? t('Notifying the SM-DP+ failed: {error}. The notification stays on the card; send it later below.', { error: tm(n.error) }) : null
}

function Progress({ job }: { job: EsimJob }) {
  return (
    <div role="status" className="space-y-1 rounded-lg border border-line/10 bg-surface2/60 p-3">
      <p className="flex min-w-0 items-center gap-2 text-[13px] font-medium text-ink">
        <Spinner size={14} className="shrink-0 text-accent" />
        <span className="min-w-0 break-words">{t(RUNNING[job.kind] ?? N_('Working…'))}</span>
      </p>
      <p className="text-xs text-ink2">{STEPS[job.step] ? t(STEPS[job.step]) : job.step}</p>
    </div>
  )
}

function Result({ job, status, onDismiss }: { job: EsimJob; status: EsimStatus; onDismiss: () => void }) {
  const ok = job.state === 'done'
  const warn = ok && job.kind === 'enable' && job.result?.firmware_ready === false
  const took = job.finished_at != null && job.finished_at >= job.started_at ? job.finished_at - job.started_at : null
  const message = ok ? doneMessage(job, status) : t(FAILED[job.kind])
  const note = ok ? notifyNote(job) : null
  const tone = warn ? 'border-warn/25 bg-warn/10' : ok ? 'border-ok/25 bg-ok/10' : 'border-danger/25 bg-danger/10'
  return (
    <div role={ok ? 'status' : 'alert'} className={`rounded-lg border p-3 ${tone}`}>
      <div className="flex items-start justify-between gap-2">
        <p className={`flex min-w-0 items-center gap-1.5 text-[13px] font-semibold ${warn ? 'text-warn' : ok ? 'text-ok' : 'text-danger'}`}>
          {ok && !warn ? <ICheck size={15} className="shrink-0" /> : <IAlert size={15} className="shrink-0" />}
          <span className="min-w-0 break-words">{message}</span>
        </p>
        <button type="button" aria-label={t('Dismiss')} onClick={onDismiss} className="-m-1 shrink-0 rounded-md p-1 text-ink3 transition-colors hover:bg-surface2 hover:text-ink">
          <IX size={14} />
        </button>
      </div>
      {!ok && job.error && <p className="mt-1 break-words text-xs text-ink">{tm(job.error)}</p>}
      {warn && typeof job.result?.detail === 'string' && <p className="mt-1 break-words text-xs text-ink">{tm(job.result.detail)}</p>}
      {note && <p className="mt-1 break-words text-xs text-ink">{note}</p>}
      {took != null && <p className="mt-1 text-[11px] text-ink3">{t('took {n} s', { n: took })}</p>}
    </div>
  )
}

function Dialog({ title, children, onClose }: { title: string; children: ReactNode; onClose: () => void }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose() }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])
  return (
    <div className="fixed inset-0 z-50 flex items-end justify-center bg-black/45 p-4 sm:items-center" onClick={onClose}>
      <div className="w-full max-w-sm rounded-xl border border-line/10 bg-surface p-4 shadow-xl" onClick={(e) => e.stopPropagation()} role="dialog" aria-modal="true">
        <h3 className="text-sm font-semibold text-ink">{title}</h3>
        {children}
      </div>
    </div>
  )
}

function RenameDialog({ profile, onSave, onClose }: { profile: EsimProfile; onSave: (name: string) => void; onClose: () => void }) {
  const [name, setName] = useState(profile.nickname)
  const bytes = new TextEncoder().encode(name).length
  const bad = bytes > 64 || /\p{Cc}/u.test(name)
  return (
    <Dialog title={t('Rename {name}', { name: label(profile) })} onClose={onClose}>
      <div className="mt-3">
        <Field label={t('Nickname')} hint={bad ? undefined : t('Shown instead of the profile name; leave empty to clear it. At most 64 bytes.')}>
          <Input aria-label={t('Nickname')} aria-invalid={bad} value={name} autoFocus onChange={(e) => setName(e.target.value)} />
        </Field>
        {bad && <p className="mt-1 text-[11px] text-danger">{t('At most 64 bytes, without control characters.')}</p>}
      </div>
      <div className="mt-4 flex justify-end gap-2">
        <Button variant="ghost" onClick={onClose}>{t('Cancel')}</Button>
        <Button variant="primary" disabled={bad || name === profile.nickname} onClick={() => onSave(name)}>{t('Save')}</Button>
      </div>
    </Dialog>
  )
}

function DeleteDialog({ profile, onDelete, onClose }: { profile: EsimProfile; onDelete: () => void; onClose: () => void }) {
  const [typed, setTyped] = useState('')
  const name = label(profile)
  return (
    <Dialog title={t('Delete {name}?', { name })} onClose={onClose}>
      <p className="mt-1.5 text-[13px] leading-relaxed text-ink2">
        {t('The profile is erased from the card and cannot be restored; installing it again needs a new activation code from the carrier. Type the profile name to confirm.')}
      </p>
      <p className="mt-2 break-all rounded-md bg-surface2 px-2 py-1 font-mono text-[12px] text-ink">{name}</p>
      <Input className="mt-2" aria-label={t('Profile name')} value={typed} autoFocus autoComplete="off" spellCheck={false} onChange={(e) => setTyped(e.target.value)} />
      <div className="mt-4 flex justify-end gap-2">
        <Button variant="ghost" onClick={onClose}>{t('Cancel')}</Button>
        <Button variant="danger" disabled={typed.trim() !== name.trim()} onClick={onDelete}>{t('Delete')}</Button>
      </div>
    </Dialog>
  )
}

function ProfileRow({ profile, locked, onEnable, onDisable, onRename, onDelete }: {
  profile: EsimProfile
  locked: boolean
  onEnable: () => void
  onDisable: () => void
  onRename: () => void
  onDelete: () => void
}) {
  const sub = [profile.nickname && profile.name ? profile.name : '', profile.provider].filter((s, i, all) => s && all.indexOf(s) === i)
  return (
    <li className="flex flex-col gap-2 py-3 sm:flex-row sm:items-center sm:justify-between">
      <div className="min-w-0">
        <div className="flex flex-wrap items-center gap-2">
          <span className="min-w-0 break-words text-[13px] font-semibold text-ink">{label(profile)}</span>
          {profile.enabled ? <Chip tone="ok">{t('Enabled')}</Chip> : <Chip>{t('Disabled')}</Chip>}
          {CLASSES[profile.class] && <Chip tone="warn">{t(CLASSES[profile.class])}</Chip>}
        </div>
        {sub.length > 0 && <p className="mt-0.5 break-words text-xs text-ink2">{sub.join(' · ')}</p>}
        <p className="mt-0.5 break-all font-mono text-[11px] text-ink3">ICCID {profile.iccid}</p>
      </div>
      <div className="flex shrink-0 flex-wrap gap-1.5">
        {profile.enabled
          ? <Button size="sm" variant="outline" disabled={locked} onClick={onDisable}>{t('Disable')}</Button>
          : <Button size="sm" variant="primary" disabled={locked} onClick={onEnable}>{t('Enable')}</Button>}
        <Button size="sm" variant="ghost" disabled={locked} onClick={onRename} aria-label={t('Rename {name}', { name: label(profile) })}>
          <IPencil size={14} />{t('Rename')}
        </Button>
        {!profile.enabled && <Button size="sm" variant="ghost" disabled={locked} onClick={onDelete} aria-label={t('Delete {name}?', { name: label(profile) })}>
          <ITrash size={14} />{t('Delete')}
        </Button>}
      </div>
    </li>
  )
}

function Notifications({ items, locked, onSend, onRemove }: {
  items: EsimNotification[]
  locked: boolean
  onSend: (seq: number[]) => void
  onRemove: (seq: number[]) => void
}) {
  const [picked, setPicked] = useState<number[]>([])
  const present = picked.filter((s) => items.some((n) => n.seq === s))
  const toggle = (seq: number) => setPicked((p) => (p.includes(seq) ? p.filter((s) => s !== seq) : [...p, seq]))
  return (
    <Card title={t('Notifications')} action={items.length > 0 && <div className="flex gap-1.5">
      <Button size="sm" variant="outline" disabled={locked || !present.length} onClick={() => onSend(present)}>{t('Send')}</Button>
      <Button size="sm" variant="ghost" disabled={locked || !present.length} onClick={() => onRemove(present)}>{t('Remove')}</Button>
    </div>}>
      <p className="text-xs text-ink2">
        {t('Notifications tell the carrier’s SM-DP+ that a profile was installed, enabled, disabled or deleted. They are sent after a download or delete; those from switching stay here until you send them.')}
      </p>
      {items.length === 0 ? <p className="mt-2 text-[13px] text-ink3">{t('No pending notifications.')}</p> : (
        <ul className="mt-2 divide-y divide-line/8">
          {items.map((n) => (
            <li key={n.seq}>
              <label className="flex cursor-pointer items-start gap-2.5 py-2">
                <input type="checkbox" className="mt-0.5" checked={picked.includes(n.seq)} onChange={() => toggle(n.seq)} disabled={locked} />
                <span className="min-w-0">
                  <span className="block text-[13px] font-medium text-ink">{OPERATIONS[n.operation] ? t(OPERATIONS[n.operation]) : n.operation} · <span className="font-mono text-[12px]">#{n.seq}</span></span>
                  <span className="block break-all text-[11px] text-ink3">{n.address} · ICCID {shortIccid(n.iccid)}</span>
                </span>
              </label>
            </li>
          ))}
        </ul>
      )}
    </Card>
  )
}

function DownloadCard({ status, locked, onDownload }: {
  status: EsimStatus
  locked: boolean
  onDownload: (body: Extract<EsimAction, { action: 'download' }>, smdp: string) => void
}) {
  const [manual, setManual] = useState(false)
  const [code, setCode] = useState('')
  const [smdp, setSmdp] = useState('')
  const [matchingId, setMatchingId] = useState('')
  const [confirmation, setConfirmation] = useState('')
  const [imei, setImei] = useState('')
  const [route, setRoute] = useState<EsimRoute>('auto')
  const [insecure, setInsecure] = useState(false)
  const [reading, setReading] = useState(false)
  const fileInput = useRef<HTMLInputElement>(null)

  const parsed = parseActivation(code)
  const codeBad = !manual && code.trim() !== '' && !parsed
  const manualBad = manual && ((smdp.trim() !== '' && !validSmdp(smdp)) || !validMatchingId(matchingId))
  const imeiBad = imei.trim() !== '' && !/^\d{15,16}$/.test(imei.trim())
  const confirmationBad = confirmation.trim() !== '' && !/^[!-~]{1,64}$/.test(confirmation.trim())
  const needsConfirmation = !manual && !!parsed?.needsConfirmation
  const ready = (manual ? validSmdp(smdp) && !manualBad : !!parsed) && !imeiBad && !confirmationBad &&
    (!needsConfirmation || confirmation.trim() !== '')
  const clashRunning = status.clash_running

  async function fromImage(file: Blob) {
    setReading(true)
    try {
      const text = await readQr(file)
      if (!text) return toast(t('No QR code found in the image.'), 'err')
      if (!parseActivation(text)) return toast(t('This QR code is not an eSIM activation code.'), 'err')
      setManual(false)
      setCode(text.trim())
      toast(t('Activation code read from the image.'))
    } catch {
      toast(t('The image could not be read.'), 'err')
    } finally {
      setReading(false)
    }
  }

  function onPaste(e: ClipboardEvent) {
    const image = Array.from(e.clipboardData.items).find((i) => i.type.startsWith('image/'))?.getAsFile()
    if (image) {
      e.preventDefault()
      void fromImage(image)
    }
  }

  async function fillDeviceImei() {
    try {
      const d = await api.simImei()
      const value = String((d as Record<string, unknown>).imei ?? '')
      if (/^\d{15,16}$/.test(value)) setImei(value)
      else toast(t('The device IMEI is unavailable.'), 'err')
    } catch (e) {
      toastError(e)
    }
  }

  function submit() {
    const extra = {
      ...(confirmation.trim() ? { confirmation_code: confirmation.trim() } : {}),
      ...(imei.trim() ? { imei: imei.trim() } : {}),
      route,
      ...(insecure ? { insecure } : {}),
    }
    if (manual) onDownload({ action: 'download', smdp: smdp.trim(), matching_id: matchingId.trim(), ...extra }, smdp.trim().toLowerCase())
    else if (parsed) onDownload({ action: 'download', activation_code: code.trim(), ...extra }, parsed.smdp)
  }

  return (
    <Card title={t('Add a profile')}>
      <div className="space-y-3" onPaste={onPaste}>
        <p className="text-xs text-ink2">
          {t('Enter the activation code from your carrier (LPA:1$…), read it from a QR code image, or enter the SM-DP+ address and activation code separately. The download uses the router’s connection: the card needs a working profile first, or Clash running.')}
        </p>
        <div className="flex flex-wrap gap-2">
          <Button size="sm" variant={manual ? 'ghost' : 'subtle'} onClick={() => setManual(false)}>{t('Activation code')}</Button>
          <Button size="sm" variant={manual ? 'subtle' : 'ghost'} onClick={() => setManual(true)}>{t('Address and code')}</Button>
          <Button size="sm" variant="outline" loading={reading} disabled={locked} onClick={() => fileInput.current?.click()}>
            <IUpload size={14} />{t('Read a QR code image…')}
          </Button>
          <input ref={fileInput} aria-label={t('QR code image')} type="file" accept="image/*" hidden
            onChange={(e) => { const file = e.target.files?.[0]; e.target.value = ''; if (file) void fromImage(file) }} />
        </div>
        {manual ? (
          <div className="grid gap-3 sm:grid-cols-2">
            <Field label={t('SM-DP+ address')}>
              <Input aria-label={t('SM-DP+ address')} aria-invalid={manualBad && smdp.trim() !== '' && !validSmdp(smdp)} autoComplete="off" spellCheck={false}
                placeholder={SMDP_EXAMPLE} value={smdp} onChange={(e) => setSmdp(e.target.value)} />
            </Field>
            <Field label={t('Activation code (matching ID)')}>
              <Input aria-label={t('Activation code (matching ID)')} aria-invalid={!validMatchingId(matchingId)} autoComplete="off" spellCheck={false}
                value={matchingId} onChange={(e) => setMatchingId(e.target.value)} />
            </Field>
          </div>
        ) : (
          <Field label={t('Activation code')} hint={parsed ? t('SM-DP+: {smdp}', { smdp: parsed.smdp }) : t('You can also paste a QR code screenshot here.')}>
            <Input aria-label={t('Activation code')} aria-invalid={codeBad} autoComplete="off" spellCheck={false}
              placeholder={CODE_EXAMPLE} value={code} onChange={(e) => setCode(e.target.value)} />
          </Field>
        )}
        {codeBad && <p className="text-[11px] text-danger">{t('Not an activation code: it starts with LPA:1$ followed by the SM-DP+ address.')}</p>}
        <Field label={needsConfirmation ? t('Confirmation code (required)') : t('Confirmation code (optional)')}
          hint={t('Some carriers give a separate confirmation code; leave empty otherwise.')}>
          <Input aria-label={t('Confirmation code')} aria-invalid={confirmationBad} autoComplete="off" spellCheck={false}
            value={confirmation} onChange={(e) => setConfirmation(e.target.value)} />
        </Field>
        <details className="rounded-lg border border-line/10 px-3 py-2">
          <summary className="cursor-pointer text-[13px] font-medium text-ink2">{t('Advanced')}</summary>
          <div className="mt-3 space-y-3">
            <div className="grid gap-3 sm:grid-cols-2">
              <Field label={t('IMEI sent to the carrier (optional)')} hint={imeiBad ? undefined : t('Only a few carriers ask for it.')}>
                <div className="flex gap-1.5">
                  <Input aria-label={t('IMEI sent to the carrier (optional)')} aria-invalid={imeiBad} inputMode="numeric" autoComplete="off"
                    value={imei} onChange={(e) => setImei(e.target.value)} />
                  <Button size="sm" variant="outline" className="h-9 shrink-0" onClick={() => void fillDeviceImei()}>{t('This device')}</Button>
                </div>
              </Field>
              <Field label={t('Connect through')} hint={t(ROUTE_HINTS[route])}>
                <Select aria-label={t('Connect through')} value={route} onChange={(e) => setRoute(e.target.value as EsimRoute)}>
                  <option value="auto">{t('Automatic')}</option>
                  <option value="proxy" disabled={!clashRunning && route !== 'proxy'}>{clashRunning ? t('Through Clash') : t('Through Clash (not running)')}</option>
                  <option value="direct">{t('Direct')}</option>
                </Select>
              </Field>
            </div>
            {imeiBad && <p className="text-[11px] text-danger">{t('An IMEI has 15 or 16 digits.')}</p>}
            <label className="flex items-start gap-2 text-[13px] text-ink2">
              <input type="checkbox" className="mt-0.5" checked={insecure} onChange={(e) => setInsecure(e.target.checked)} />
              <span>{t('Do not verify the server certificate. Only for an SM-DP+ whose certificate the router does not trust; the activation code is then not protected in transit.')}</span>
            </label>
          </div>
        </details>
        <div className="flex justify-end">
          <Button variant="primary" disabled={locked || !ready} onClick={submit}>{t('Download')}</Button>
        </div>
      </div>
    </Card>
  )
}

/** Profiles on a removable eUICC: read, switch, rename, delete, download, notifications. */
export default function EsimTab() {
  const [status, setStatus] = useState<EsimStatus | null>(null)
  const [error, setError] = useState('')
  const [oldAgent, setOldAgent] = useState(false)
  const [sending, setSending] = useState(false)
  const [watched, setWatched] = useState('')
  const [dismissed, setDismissed] = useState('')
  const [renaming, setRenaming] = useState<EsimProfile | null>(null)
  const [deleting, setDeleting] = useState<EsimProfile | null>(null)
  const [reads, setReads] = useState(0)
  const mounted = useRef(true)
  const seen = useRef<EsimJob | null>(null)
  const autoScanned = useRef(false)
  const issued = useRef(0)
  const applied = useRef(0)

  const finished = useCallback((job: EsimJob, s: EsimStatus) => {
    if (job.state === 'failed') toast(t('{name}: {reason}', { name: t(FAILED[job.kind]), reason: tm(job.error) ?? '' }), 'err')
    else {
      const message = doneMessage(job, s)
      if (message) toast(message, job.kind === 'enable' && job.result?.firmware_ready === false ? 'err' : 'ok')
    }
  }, [])

  /** Shows a status read or an action's answer; `started` marks a job this tab started. */
  const apply = useCallback((s: EsimStatus, ticket: number, started = false) => {
    if (ticket < applied.current) return
    applied.current = ticket
    const job = s.job
    const before = seen.current
    seen.current = job
    for (const p of s.view?.profiles ?? []) knownNames.set(p.iccid, label(p))
    setStatus(s); setError(''); setOldAgent(false)
    if (!job || !(job.kind in RUNNING)) return
    const id = jobId(job)
    if (job.state === 'running') setWatched(id)
    else if (started || (before?.state === 'running' && jobId(before) === id)) {
      setWatched(id)
      finished(job, s)
    }
  }, [finished])

  const load = useCallback(async () => {
    const ticket = ++issued.current
    try {
      const s = await api.esim()
      if (mounted.current) apply(s, ticket)
    } catch (e) {
      if (!mounted.current) return
      if (e instanceof ApiError && e.status === 404) setOldAgent(true)
      else setError(e instanceof Error ? e.message : t('Unable to load the eSIM status'))
    } finally {
      if (mounted.current) setReads((n) => n + 1)
    }
  }, [apply])

  useEffect(() => {
    mounted.current = true
    void load()
    return () => { mounted.current = false }
  }, [load])

  const job = status?.job ?? null
  const running = job?.state === 'running'
  const retry = !status && !oldAgent
  useEffect(() => {
    if (!running && !retry) return
    const timer = window.setTimeout(() => void load(), running ? POLL_MS : RETRY_MS)
    return () => window.clearTimeout(timer)
  }, [running, retry, reads, load])

  const start = useCallback(async (call: () => Promise<EsimStatus>, failure: string) => {
    const ticket = ++issued.current
    setSending(true)
    try {
      const s = await call()
      if (mounted.current) apply(s, ticket, true)
    } catch (e) {
      toastError(e, failure)
      void load()
    } finally {
      if (mounted.current) setSending(false)
    }
  }, [apply, load])

  // Reading changes nothing on the card: do it once when the tab opens without a read.
  useEffect(() => {
    if (!status || autoScanned.current || !status.installed || status.view || status.job?.state === 'running') return
    autoScanned.current = true
    void start(() => api.esimAction({ action: 'scan' }), t(FAILED.scan))
  }, [status, start])

  const locked = !status || sending || running
  const view = status?.view ?? null

  async function enable(p: EsimProfile) {
    if (!(await confirm({
      title: t('Switch to {name}?', { name: label(p) }),
      body: t('Mobile data stops while the modem reads the card again, usually for 10–30 seconds. If you reach this page through the router’s own mobile connection, you may lose access. If the firmware keeps using the previous profile, you will be asked to restart the device.'),
      confirmLabel: t('Switch'),
    }))) return
    await start(() => api.esimAction({ action: 'enable', iccid: p.iccid }), t(FAILED.enable))
  }

  async function disable(p: EsimProfile) {
    if (!(await confirm({
      title: t('Disable {name}?', { name: label(p) }),
      body: t('The card is left without an active profile: the router has no mobile service until you enable one.'),
      confirmLabel: t('Disable'),
      danger: true,
    }))) return
    await start(() => api.esimAction({ action: 'disable', iccid: p.iccid }), t(FAILED.disable))
  }

  async function download(body: Extract<EsimAction, { action: 'download' }>, smdp: string) {
    if (!(await confirm({
      title: t('Download a profile from {smdp}?', { smdp }),
      body: t('The profile is added to the card disabled; enable it afterwards. An activation code can often be used only once: if a download fails, read the card before trying again.'),
      confirmLabel: t('Download'),
    }))) return
    await start(() => api.esimAction(body), t(FAILED.download))
  }

  async function removeNotifications(seq: number[]) {
    if (!(await confirm({
      title: t('Remove {n} notifications without sending them?', { n: seq.length }),
      body: t('The carrier’s SM-DP+ is then not told about these changes.'),
      confirmLabel: t('Remove'),
      danger: true,
    }))) return
    await start(() => api.esimAction({ action: 'notifications', operation: 'remove', seq }), t(FAILED.notifications))
  }

  async function reboot() {
    if (!(await confirm({
      title: t('Restart the device?'),
      body: t('The router restarts and is unreachable for about two minutes.'),
      confirmLabel: t('Restart'),
      danger: true,
    }))) return
    try {
      await api.reboot()
      toast(t('Restarting…'))
    } catch (e) {
      toastError(e)
    }
  }

  function jobPanel() {
    if (!status || !job) return null
    if (job.state === 'running') return <Progress job={job} />
    const id = jobId(job)
    if (id === dismissed || (job.state === 'done' && (id !== watched || job.kind === 'scan'))) return null
    return <Result job={job} status={status} onDismiss={() => setDismissed(id)} />
  }

  if (oldAgent) return <Card title="eSIM"><p className="text-sm text-ink2">{t(OLD_AGENT)}</p></Card>
  if (!status) {
    return <Card title="eSIM">{error
      ? <p role="alert" className="rounded-lg bg-danger/10 p-3 text-[13px] text-danger">{tm(error)}</p>
      : <p className="text-sm text-ink2">{t('Loading…')}</p>}</Card>
  }
  if (!status.installed) {
    return (
      <Card title="eSIM">
        <p className="text-sm text-ink2">
          {t('lpac is not installed on the device. Deploy it with {command}.').split('{command}')
            .flatMap((part, i) => i ? [<code key={i}>scripts/deploy-components.py --lpac</code>, part] : [part])}
        </p>
      </Card>
    )
  }

  const scanButton = (
    <Button size="sm" variant="outline" disabled={locked} onClick={() => void start(() => api.esimAction({ action: 'scan' }), t(FAILED.scan))}>
      <IRefresh size={14} />{view ? t('Read again') : t('Read the card')}
    </Button>
  )
  const enabledProfile = view?.profiles.find((p) => p.enabled)
  const firmwareIccid = view?.firmware?.iccid ?? ''
  const mismatch = !!enabledProfile && !!view?.firmware && firmwareIccid !== '' && firmwareIccid !== normalIccid(enabledProfile.iccid)

  return (
    <div className="space-y-4">
      {error && <p role="alert" className="rounded-lg bg-danger/10 p-3 text-[13px] text-danger">{tm(error)}</p>}
      {jobPanel()}
      {status.reboot_needed && (
        <div role="alert" className="flex flex-col gap-2 rounded-lg border border-warn/25 bg-warn/10 p-3 sm:flex-row sm:items-center sm:justify-between">
          <p className="text-[13px] text-ink">{t('The card has switched profiles, but the firmware still uses the previous one. Restart the device to finish.')}</p>
          <Button size="sm" variant="primary" className="shrink-0" onClick={() => void reboot()}>{t('Restart the device')}</Button>
        </div>
      )}

      <Card title={t('eSIM card')} action={scanButton}>
        {!view ? (
          <p className="text-sm text-ink2">{t('Manage profiles on a removable eUICC card (9eSIM, 5ber, eSTK.me and similar) in the SIM tray, as EasyEUICC does on Android. Reading the card changes nothing.')}</p>
        ) : view.card === 'euicc' ? (
          <div className="space-y-1">
            <div className="flex items-baseline justify-between gap-3 py-0.5 text-[13px]">
              <span className="shrink-0 text-ink2">EID</span>
              <span className="min-w-0 break-all text-right font-mono text-[12px] font-medium text-ink">{view.eid || '—'}</span>
            </div>
            {view.info?.sgp22 && <InfoRow name="SGP.22" value={String(view.info.sgp22)} />}
            {view.info?.firmware && <InfoRow name={t('eUICC firmware')} value={String(view.info.firmware)} />}
            {view.info?.free_nvm != null && <InfoRow name={t('Free space on the card')} value={formatBytes(Number(view.info.free_nvm))} />}
            {view.info?.default_smdp && <InfoRow name={t('Default SM-DP+')} value={String(view.info.default_smdp)} />}
            <InfoRow name={t('Firmware is using')} value={view.firmware
              ? (firmwareIccid ? `ICCID ${shortIccid(firmwareIccid)}` : t('No SIM')) + (view.firmware.state ? ` · ${view.firmware.state}` : '')
              : '—'} />
            {mismatch && !status.reboot_needed && <p className="pt-1 text-[11px] text-warn">{t('The firmware reports a different ICCID than the enabled profile. Restart the device if mobile data does not work.')}</p>}
          </div>
        ) : view.card === 'sim' ? (
          <Empty icon={<ISim size={28} />} title={t('An ordinary SIM card is in the tray')} body={t('Profiles can only be downloaded to an eUICC card such as 9eSIM, 5ber or eSTK.me.')} />
        ) : view.card === 'absent' ? (
          <Empty icon={<ISim size={28} />} title={t('No card in the SIM tray')} body={t('Insert a removable eUICC card, then read the card again.')} />
        ) : (
          <p role="alert" className="rounded-lg bg-danger/10 p-3 text-[13px] text-danger">{tm(view.detail) || t('The card could not be read.')}</p>
        )}
      </Card>

      {view?.card === 'euicc' && <>
        <Card title={t('Profiles')}>
          {view.profiles.length === 0
            ? <Empty icon={<ISim size={28} />} title={t('No profiles on the card')} body={t('Add one below with an activation code from your carrier.')} />
            : <ul className="-my-3 divide-y divide-line/8">
              {view.profiles.map((p) => (
                <ProfileRow key={p.iccid} profile={p} locked={locked}
                  onEnable={() => void enable(p)} onDisable={() => void disable(p)}
                  onRename={() => setRenaming(p)} onDelete={() => setDeleting(p)} />
              ))}
            </ul>}
        </Card>
        <DownloadCard status={status} locked={locked} onDownload={(body, smdp) => void download(body, smdp)} />
        <Notifications items={view.notifications} locked={locked}
          onSend={(seq) => void start(() => api.esimAction({ action: 'notifications', operation: 'process', seq }), t(FAILED.notifications))}
          onRemove={(seq) => void removeNotifications(seq)} />
      </>}

      {renaming && <RenameDialog profile={renaming} onClose={() => setRenaming(null)} onSave={(nickname) => {
        const p = renaming
        setRenaming(null)
        void start(() => api.esimAction({ action: 'nickname', iccid: p.iccid, nickname }), t(FAILED.nickname))
      }} />}
      {deleting && <DeleteDialog profile={deleting} onClose={() => setDeleting(null)} onDelete={() => {
        const p = deleting
        setDeleting(null)
        void start(() => api.esimDelete(p.iccid), t(FAILED.delete))
      }} />}
    </div>
  )
}

function InfoRow({ name, value }: { name: string; value: string }) {
  return (
    <div className="flex items-baseline justify-between gap-3 py-0.5 text-[13px]">
      <span className="shrink-0 text-ink2">{name}</span>
      <span className="min-w-0 break-all text-right font-medium text-ink">{value}</span>
    </div>
  )
}
