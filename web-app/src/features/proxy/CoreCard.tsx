import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react'
import { api } from '../../data/api'
import { ApiError } from '../../data/client'
import { formatBytes } from '../../format'
import { IAlert, ICheck, IX } from '../../icons'
import type { ClashCore, ClashCoreAction, ClashCoreBuild, ClashCoreJob, ClashCoreJobKind, ClashCoreRoute, ClashCoreSource, ClashCoreStep } from '../../types'
import { Button, Field, Input, Select } from '../../ui/controls'
import { confirm, toast, toastError } from '../../ui/feedback'
import { Card, Chip, Meter, Spinner, type ChipTone } from '../../ui/primitives'
import { useClash } from './ClashContext'
import { useProxy } from './ProxyContext'
import { N_, locale, t, tm } from '../../i18n'

// Clash core and geo data updates (agent/src/clash_core.rs). Each action starts a job on the
// agent and answers at once; the card follows the job until it ends. Device times may be
// hours off, so only durations are shown.

const POLL_MS = 1500
const RETRY_MS = 5000
const MIB = 1024 * 1024
/** The agent's upload limit and the room it keeps free on /data. */
const MAX_UPLOAD = 64 * MIB
const MARGIN = 16 * MIB

const SOURCES: Record<ClashCoreSource, { label: string; tone: ChipTone }> = {
  pinned: { label: N_('Pinned build'), tone: 'default' },
  official: { label: N_('Official release'), tone: 'accent' },
  uploaded: { label: N_('Uploaded'), tone: 'nr' },
  unknown: { label: N_('Unknown'), tone: 'default' },
}
const ROUTE_HINTS: Record<ClashCoreRoute, string> = {
  auto: N_('Through the Clash core while it runs, else directly.'),
  proxy: N_('Through the running Clash core and its selected node.'),
  direct: N_('Straight from GitHub, which is often slow or blocked in mainland China.'),
  mirror: N_('Core and geo files come through the mirror; release data and checksums still come from GitHub.'),
}
const STEPS: Record<ClashCoreStep, string> = {
  checking: N_('Asking GitHub for the latest release'),
  receiving: N_('Receiving the file'),
  downloading: N_('Downloading'),
  verifying: N_('Verifying the checksum'),
  extracting: N_('Extracting'),
  testing: N_('Testing with the current profile'),
  installing: N_('Installing'),
  restarting: N_('Restarting the core'),
}
/** The steps of each job in order, for its overall progress; some may be skipped. */
const SEQUENCES: Record<ClashCoreJobKind, ClashCoreStep[]> = {
  check: ['checking'],
  update: ['downloading', 'verifying', 'extracting', 'testing', 'installing', 'restarting'],
  upload: ['receiving', 'extracting', 'testing', 'installing', 'restarting'],
  rollback: ['testing', 'installing', 'restarting'],
  geodata: ['downloading', 'verifying', 'testing', 'installing', 'restarting'],
}
const RUNNING: Record<ClashCoreJobKind, string> = {
  check: N_('Checking for updates…'),
  update: N_('Updating the core…'),
  upload: N_('Installing the uploaded core…'),
  rollback: N_('Rolling back the core…'),
  geodata: N_('Updating geo data…'),
}
const FAILED: Record<ClashCoreJobKind, string> = {
  check: N_('Update check failed'),
  update: N_('Core update failed'),
  upload: N_('Upload failed'),
  rollback: N_('Rollback failed'),
  geodata: N_('Geo data update failed'),
}

/** Agents older than core updates answer 404 "not found". */
const OLD_AGENT = N_('This agent cannot update the core; deploy the latest agent.')

const jobId = (j: ClashCoreJob) => `${j.kind}:${j.started_at}`
const versionOf = (b: ClashCoreBuild | null) => (b ? b.version || t('Unknown version') : '—')

/** What the agent accepts: an https:// prefix with a plain host, at most 200 characters. */
function validMirror(m: string) {
  const host = m.startsWith('https://') ? m.slice(8).split('/')[0] : ''
  return m.length <= 200 && /^[A-Za-z0-9.:-]+$/.test(host) && /^[!-~]+$/.test(m) && !/["\\?#@]/.test(m)
}

function published(iso: string) {
  const date = new Date(iso)
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleDateString(locale())
}

function doneMessage(kind: ClashCoreJobKind, c: ClashCore) {
  const version = versionOf(c.installed)
  switch (kind) {
    case 'check': return c.latest && c.installed?.version === c.latest.tag
      ? t('The core is up to date.') : t('Latest release: {tag}.', { tag: c.latest?.tag ?? '—' })
    case 'update': return t('Core updated to {version}.', { version })
    case 'upload': return t('Uploaded core installed: {version}.', { version })
    case 'rollback': return t('Rolled back to {version}.', { version })
    case 'geodata': return t('Geo data updated.')
  }
}

function Build({ title, build, action }: { title: string; build: ClashCoreBuild | null; action?: ReactNode }) {
  const source = build ? SOURCES[build.source] ?? SOURCES.unknown : null
  return (
    <div className="min-w-0 rounded-lg border border-line/10 p-3">
      <div className="flex min-h-8 items-center justify-between gap-2">
        <p className="text-[11px] font-semibold uppercase tracking-wider text-ink3">{title}</p>
        {action}
      </div>
      {build && source ? <>
        <div className="mt-1 flex flex-wrap items-center gap-2">
          <span className="min-w-0 break-all font-mono text-[13px] font-semibold text-ink">{versionOf(build)}</span>
          <Chip tone={source.tone}>{t(source.label)}</Chip>
        </div>
        {build.sha256 && <p className="mt-1 truncate font-mono text-[11px] text-ink3" title={build.sha256}>SHA-256 {build.sha256.slice(0, 16)}…</p>}
      </> : <p className="mt-1 text-[13px] text-ink2">{t('Not installed')}</p>}
    </div>
  )
}

function Section({ title, action, children }: { title: string; action?: ReactNode; children: ReactNode }) {
  return (
    <section className="space-y-2 border-t border-line/8 pt-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="text-[13px] font-semibold text-ink">{title}</h3>
        {action}
      </div>
      {children}
    </section>
  )
}

function Progress({ title, detail, pct }: { title: string; detail?: string; pct: number | null }) {
  return (
    <div role="status" className="space-y-2 rounded-lg border border-line/10 bg-surface2/60 p-3">
      <p className="flex min-w-0 items-center gap-2 text-[13px] font-medium text-ink">
        <Spinner size={14} className="shrink-0 text-accent" />
        <span className="min-w-0 break-words">{title}</span>
      </p>
      {detail && <p className="text-xs text-ink2">{detail}</p>}
      {pct != null && <Meter pct={pct} />}
    </div>
  )
}

function Running({ job }: { job: ClashCoreJob }) {
  const steps = SEQUENCES[job.kind] ?? [job.step]
  const part = job.total > 0 ? Math.min(1, job.bytes / job.total) : 0
  const amount = job.total > 0 ? t('{done} of {total}', { done: formatBytes(job.bytes), total: formatBytes(job.total) })
    : job.bytes > 0 ? formatBytes(job.bytes) : ''
  const detail = [STEPS[job.step] ? t(STEPS[job.step]) : job.step, amount].filter(Boolean).join(' · ')
  const pct = steps.length > 1 ? ((Math.max(0, steps.indexOf(job.step)) + part) / steps.length) * 100 : null
  return <Progress title={t(RUNNING[job.kind])} detail={detail} pct={pct} />
}

function Result({ job, core, onDismiss }: { job: ClashCoreJob; core: ClashCore; onDismiss: () => void }) {
  const ok = job.state === 'done'
  const took = job.finished_at != null && job.finished_at >= job.started_at ? job.finished_at - job.started_at : null
  return (
    <div role={ok ? 'status' : 'alert'} className={`rounded-lg border p-3 ${ok ? 'border-ok/25 bg-ok/10' : 'border-danger/25 bg-danger/10'}`}>
      <div className="flex items-start justify-between gap-2">
        <p className={`flex min-w-0 items-center gap-1.5 text-[13px] font-semibold ${ok ? 'text-ok' : 'text-danger'}`}>
          {ok ? <ICheck size={15} className="shrink-0" /> : <IAlert size={15} className="shrink-0" />}
          <span className="min-w-0 break-words">{ok ? doneMessage(job.kind, core) : t(FAILED[job.kind])}</span>
        </p>
        <button type="button" aria-label={t('Dismiss')} onClick={onDismiss} className="-m-1 shrink-0 rounded-md p-1 text-ink3 transition-colors hover:bg-surface2 hover:text-ink">
          <IX size={14} />
        </button>
      </div>
      {!ok && job.error && <p className="mt-1 break-words text-xs text-ink">{tm(job.error)}</p>}
      {!ok && core.mirror_hint && <p className="mt-1 text-xs text-ink2">{t('GitHub could not be reached. Choose a mirror or Through Clash under Download through.')}</p>}
      {took != null && <p className="mt-1 text-[11px] text-ink3">{t('took {n} s', { n: took })}</p>}
    </div>
  )
}

/** The installed Clash core, its updates, rollback, upload and the geo databases. */
export default function CoreCard() {
  const { status, refresh: refreshClash } = useClash()
  const { refresh: refreshProxy } = useProxy()
  const [core, setCore] = useState<ClashCore | null>(null)
  const [error, setError] = useState('')
  const [oldAgent, setOldAgent] = useState(false)
  const [sending, setSending] = useState(false)
  const [uploading, setUploading] = useState<{ name: string; sent: number; total: number } | null>(null)
  // The job this page started or saw running: its outcome is shown when it ends.
  const [watched, setWatched] = useState('')
  const [dismissed, setDismissed] = useState('')
  const [route, setRoute] = useState<ClashCoreRoute>('auto')
  const [mirror, setMirror] = useState('')
  // Counts finished reads, so the next poll is scheduled after each one, failed or not.
  const [reads, setReads] = useState(0)
  const mounted = useRef(true)
  const seen = useRef<ClashCoreJob | null>(null)
  const fileInput = useRef<HTMLInputElement>(null)
  // Requests are numbered so a slow read cannot overwrite what a later action returned.
  const issued = useRef(0)
  const applied = useRef(0)

  const finished = useCallback((job: ClashCoreJob, c: ClashCore) => {
    if (job.state === 'failed') toast(t('{name}: {reason}', { name: t(FAILED[job.kind]), reason: tm(job.error) ?? '' }), 'err')
    else toast(doneMessage(job.kind, c))
    // The core, its version or the geo data changed: refresh the overview and readiness.
    if (job.kind !== 'check') { void refreshClash(); void refreshProxy() }
  }, [refreshClash, refreshProxy])

  /** Shows a status read or an action's answer; `started` marks a job this page started. */
  const apply = useCallback((c: ClashCore, ticket: number, started = false) => {
    if (ticket < applied.current) return
    applied.current = ticket
    const job = c.job
    const before = seen.current
    seen.current = job
    setCore(c); setError(''); setOldAgent(false)
    // A job kind this page does not know (a newer agent) only locks the actions.
    if (!job || !(job.kind in RUNNING)) return
    const id = jobId(job)
    if (job.state === 'running') setWatched(id)
    else if (started || (before?.state === 'running' && jobId(before) === id)) {
      setWatched(id)
      finished(job, c)
    }
  }, [finished])

  const load = useCallback(async () => {
    const ticket = ++issued.current
    try {
      const c = await api.clashCore()
      if (mounted.current) apply(c, ticket)
    } catch (e) {
      if (!mounted.current) return
      if (e instanceof ApiError && e.status === 404) setOldAgent(true)
      else setError(e instanceof Error ? e.message : t('Unable to load the core status'))
    } finally {
      if (mounted.current) setReads(n => n + 1)
    }
  }, [apply])

  useEffect(() => {
    mounted.current = true
    void load()
    return () => { mounted.current = false }
  }, [load])

  const job = core?.job ?? null
  const running = job?.state === 'running'
  // Follows a running job; a first read that failed is retried more slowly.
  const retry = !core && !oldAgent
  useEffect(() => {
    if (!running && !retry) return
    const timer = window.setTimeout(() => void load(), running ? POLL_MS : RETRY_MS)
    return () => window.clearTimeout(timer)
  }, [running, retry, reads, load])

  const locked = !core || sending || !!uploading || running
  const clashRunning = !!status?.running
  const mirrorText = mirror.trim()
  const mirrorBad = route === 'mirror' && !!mirrorText && !validMirror(mirrorText)
  const mirrorReady = route !== 'mirror' || validMirror(mirrorText)
  const via = route === 'mirror' ? { route, mirror: mirrorText } : { route }

  async function start(action: ClashCoreAction, failure: string) {
    const ticket = ++issued.current
    setSending(true)
    try {
      const c = await api.clashCoreAction(action)
      if (mounted.current) apply(c, ticket, true)
    } catch (e) {
      toastError(e, failure)
      // Another job may be running (409): show it.
      void load()
    } finally {
      if (mounted.current) setSending(false)
    }
  }

  function check() {
    // The mirror only carries files; release data always comes from GitHub.
    void start({ action: 'check', route: route === 'mirror' ? 'auto' : route }, t(FAILED.check))
  }

  async function update() {
    const latest = core?.latest
    if (!latest) return
    if (!(await confirm({
      title: t('Update the core to {tag}?', { tag: latest.tag }),
      body: t('Downloads {size}, checks it against the checksum GitHub publishes and tests it with the current profile. With Clash on, the core restarts and proxied devices lose connectivity for a few seconds; if the new core does not start, the previous one is restored automatically. Deploying later with the deploy script installs the pinned build again.', { size: formatBytes(latest.size) }),
      confirmLabel: t('Update'),
    }))) return
    await start({ action: 'update', tag: latest.tag, ...via }, t(FAILED.update))
  }

  async function rollback() {
    const previous = core?.previous
    if (!previous) return
    if (!(await confirm({
      title: t('Roll back to {version}?', { version: versionOf(previous) }),
      body: t('The previous core is tested with the current profile, then the two are swapped, so you can switch back later. With Clash on, the core restarts and proxied devices lose connectivity for a few seconds; if it does not start, the current core is restored.'),
      confirmLabel: t('Roll back'),
    }))) return
    await start({ action: 'rollback' }, t(FAILED.rollback))
  }

  async function geodata() {
    if (!(await confirm({
      title: t('Update geo data?'),
      body: t('Downloads GeoIP and GeoSite and checks them against their published checksums. With Clash on, the core restarts to load them, since it caches the databases, and proxied devices lose connectivity for a few seconds; if it does not start, the previous data is restored.'),
      confirmLabel: t('Update'),
    }))) return
    await start({ action: 'geodata', ...via }, t(FAILED.geodata))
  }

  async function upload(file: File) {
    // The agent refuses these too, but only after the whole file has been sent.
    if (!file.size) return toast(tm('Upload is empty'), 'err')
    if (file.size > MAX_UPLOAD) return toast(tm('Upload exceeds 64 MiB'), 'err')
    if (core && file.size + MARGIN > core.free_bytes) return toast(tm('Not enough free space on /data'), 'err')
    if (!(await confirm({
      title: t('Install {name}?', { name: file.name }),
      body: t('Uploads {size}. The file must be a mihomo build for arm64 Linux; it is tested with the current profile before it replaces the installed core. With Clash on, the core restarts and proxied devices lose connectivity for a few seconds; if the new core does not start, the previous one is restored automatically.', { size: formatBytes(file.size) }),
      confirmLabel: t('Upload & install'),
    }))) return
    const ticket = ++issued.current
    setUploading({ name: file.name, sent: 0, total: file.size })
    try {
      const c = await api.clashCoreUpload(file, (sent, total) => {
        if (mounted.current) setUploading({ name: file.name, sent, total })
      })
      if (mounted.current) apply(c, ticket, true)
    } catch (e) {
      toastError(e, t(FAILED.upload))
      void load()
    } finally {
      if (mounted.current) setUploading(null)
    }
  }

  /** The running job, or the outcome of one this page followed (failures always), for `kinds`. */
  function jobPanel(...kinds: ClashCoreJobKind[]) {
    if (!core || !job || !kinds.includes(job.kind)) return null
    if (job.state === 'running') return <Running job={job} />
    const id = jobId(job)
    // A finished check shows as the latest release; other successes only when followed here.
    if (id === dismissed || (job.state === 'done' && (id !== watched || job.kind === 'check'))) return null
    return <Result job={job} core={core} onDismiss={() => setDismissed(id)} />
  }

  if (oldAgent) {
    return <Card title={t('Core and data')}><p className="text-sm text-ink2">{t(OLD_AGENT)}</p></Card>
  }

  const latest = core?.latest
  const upToDate = !!latest && core?.installed?.version === latest.tag
  const geodataFiles = status?.geodata ?? []

  return (
    <Card title={t('Core and data')}>
      {!core ? (error
        ? <p role="alert" className="rounded-lg bg-danger/10 p-3 text-[13px] text-danger">{tm(error)}</p>
        : <p className="text-sm text-ink2">{t('Loading…')}</p>
      ) : <div className="space-y-4">
        <p className="text-xs text-ink2">{t('Replace the Clash core or its geo data here. With Clash on, each change restarts the core and proxied devices lose connectivity for a few seconds.')}</p>
        {error && <p role="alert" className="rounded-lg bg-danger/10 p-3 text-[13px] text-danger">{tm(error)}</p>}

        <div className="space-y-2">
          <div className="grid gap-3 sm:grid-cols-2">
            <Build title={t('Installed core')} build={core.installed} />
            {core.previous && <Build title={t('Previous core')} build={core.previous}
              action={<Button size="sm" variant="outline" disabled={locked} onClick={() => void rollback()}>{t('Roll back')}</Button>} />}
          </div>
          {jobPanel('rollback')}
          <p className="text-[11px] text-ink3">{t('Free space on /data: {size}', { size: core.free_bytes ? formatBytes(core.free_bytes) : '—' })}</p>
        </div>

        <div className="space-y-2 border-t border-line/8 pt-4">
          <div className="grid gap-3 sm:grid-cols-2">
            <Field label={t('Download through')} hint={t(ROUTE_HINTS[route])}>
              <Select aria-label={t('Download through')} value={route} onChange={e => setRoute(e.target.value as ClashCoreRoute)}>
                <option value="auto">{t('Automatic')}</option>
                <option value="proxy" disabled={!clashRunning && route !== 'proxy'}>{clashRunning ? t('Through Clash') : t('Through Clash (not running)')}</option>
                <option value="direct">{t('Direct')}</option>
                <option value="mirror">{t('Mirror')}</option>
              </Select>
            </Field>
            {(route === 'mirror' || core.mirror_hint) && <div>
              <Field label={t('Mirror prefix')} hint={mirrorBad ? undefined : route === 'mirror'
                ? t('Put before the GitHub address, for example https://ghfast.top/')
                : t('The last download from GitHub failed. Enter a mirror to download through it.')}>
                <Input aria-label={t('Mirror prefix')} aria-invalid={mirrorBad} type="url" inputMode="url" autoComplete="off" spellCheck={false}
                  placeholder="https://ghfast.top/" value={mirror}
                  onChange={e => { setMirror(e.target.value); if (e.target.value.trim()) setRoute('mirror') }} />
              </Field>
              {mirrorBad && <p className="mt-1 text-[11px] text-danger">{t('Enter an https:// address prefix, for example https://ghfast.top/')}</p>}
            </div>}
          </div>
          <p className="text-[11px] text-ink3">{t('Used to check for updates, download the core and update geo data.')}</p>
        </div>

        <Section title={t('Official release')} action={
          <Button size="sm" variant="outline" disabled={locked} onClick={check}>{t('Check for updates')}</Button>}>
          {latest ? <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
            <span className="font-mono text-[13px] font-semibold text-ink">{latest.tag}</span>
            <span className="text-xs text-ink2">{t('Published {date}', { date: published(latest.published_at) })} · {formatBytes(latest.size)}</span>
            {upToDate
              ? <Chip tone="ok">{t('Up to date')}</Chip>
              : <Button size="sm" variant="primary" disabled={locked || !mirrorReady} onClick={() => void update()}>{t('Update to {tag}', { tag: latest.tag })}</Button>}
          </div> : <p className="text-xs text-ink2">{t('Not checked yet.')}</p>}
          <p className="text-xs text-ink3">{t('The latest MetaCubeX release for arm64 Linux, from GitHub. Checks are cached for 10 minutes.')}</p>
          {jobPanel('check', 'update')}
        </Section>

        <Section title={t('Geo data')} action={
          <Button size="sm" variant="outline" disabled={locked || !mirrorReady} onClick={() => void geodata()}>{t('Update GeoIP / GeoSite')}</Button>}>
          <p className="text-xs text-ink2">{geodataFiles.length ? t('Installed: {files}', { files: geodataFiles.join(', ') }) : t('Not installed')}</p>
          <p className="text-xs text-ink3">{t('From MetaCubeX/meta-rules-dat, checked against the published checksums. GEOIP and GEOSITE rules need them; the core caches them, so it restarts to load new ones.')}</p>
          {jobPanel('geodata')}
        </Section>

        <Section title={t('Upload a core')} action={
          <Button size="sm" variant="outline" disabled={locked} onClick={() => fileInput.current?.click()}>{t('Choose a file…')}</Button>}>
          <p className="text-xs text-ink2">{t('A mihomo build for arm64 Linux, as a .gz archive or the executable, up to 64 MiB. It is checked and tested like an update.')}</p>
          <input ref={fileInput} aria-label={t('Core file')} type="file" hidden disabled={locked}
            onChange={e => { const file = e.target.files?.[0]; e.target.value = ''; if (file) void upload(file) }} />
          {uploading && <Progress title={t('Uploading {name}…', { name: uploading.name })}
            detail={t('{done} of {total}', { done: formatBytes(uploading.sent), total: formatBytes(uploading.total) })}
            pct={uploading.total ? (uploading.sent / uploading.total) * 100 : 0} />}
          {!uploading && jobPanel('upload')}
        </Section>

        <p className="border-t border-line/8 pt-4 text-[11px] text-ink3">
          {t('Deploying with {command} installs the pinned build again, replacing a core updated or uploaded here.').split('{command}')
            .flatMap((part, i) => i ? [<code key={i}>scripts/deploy-components.py --mihomo</code>, part] : [part])}
        </p>
      </div>}
    </Card>
  )
}
