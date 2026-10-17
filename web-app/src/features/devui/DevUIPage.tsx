import { useEffect, useRef, useState } from 'react'
import { api } from '../../data/api'
import type { DevUISettings, DevUIPageId, ScreenStatus } from '../../types'
import { Button, Select, Toggle } from '../../ui/controls'
import { Card, Chip, PageHeader } from '../../ui/primitives'
import { confirm } from '../../ui/feedback'
import { N_, t, tm } from '../../i18n'

const pages: { id: DevUIPageId; title: string; description: string }[] = [
  { id: 'overview', title: N_('Overview'), description: N_('Battery, temperature, uptime and system load') },
  { id: 'proxy', title: N_('Proxy'), description: N_('Choose Off, WireGuard or Clash and run diagnostics') },
  { id: 'wireguard', title: N_('WireGuard'), description: N_('Tunnel status, traffic and on / off control') },
  { id: 'clash', title: N_('Clash'), description: N_('Status, proxy mode, nodes and delay tests, subscriptions, core and geo data') },
  { id: 'esim', title: N_('eSIM'), description: N_('Profiles on a removable eUICC: switch, or restart after a switch') },
  { id: 'uplink', title: N_('Wi-Fi relay'), description: N_('Upstream Wi-Fi status, signal and traffic, with on / off control') },
  { id: 'wifi', title: N_('Wi-Fi'), description: N_('2.4 GHz & 5 GHz radio switches, SSID and connected client count') },
  { id: 'signal', title: N_('Signal'), description: N_('Live RSRP / SINR / band metrics and 5G SA / NSA / 4G mode switch') },
  { id: 'usb', title: N_('USB & TTL'), description: N_('RNDIS / NCM / ECM USB mode and TTL 64 / 65 / 128') },
  { id: 'power', title: N_('Battery & Charge'), description: N_('Real-time voltage / current / health and 70% / 80% / 90% / 100% charge limits') },
  { id: 'booster', title: N_('Booster'), description: N_('One-tap ZTE bloatware cleanup and Linux pagecache drop for lower load') },
  { id: 'profiles', title: N_('Saved profiles'), description: N_('Switch between your saved VPN profiles') },
  { id: 'devices', title: N_('Device routing'), description: N_('Choose which devices use WireGuard') },
]
/** Up to 9 features fit the 3×3 grid (1–4 items render as list rows). */
const MAX_MENU_ITEMS = 9
const GRID_PRESET: DevUIPageId[] = ['overview', 'proxy', 'wifi', 'signal', 'usb', 'power', 'booster', 'clash', 'esim']
const CLASSIC_PRESET: DevUIPageId[] = ['overview', 'proxy', 'clash', 'esim']
// i18n-ignore: each language is named in its own language
const LANGUAGES = { zh: '中文', en: 'English' }
const labels: Record<string, string> = { idle: N_('Stock UI'), starting: N_('Opening'), active: N_('DevUI active'), restoring: N_('Restoring'), error: N_('Recovery needs attention') }
export default function DevUIPage() {
  const [status, setStatus] = useState<ScreenStatus | null>(null)
  const [settings, setSettings] = useState<DevUISettings | null>(null)
  const [draft, setDraft] = useState<DevUISettings | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [loadError, setLoadError] = useState('')
  const [message, setMessage] = useState('')
  const mutating = useRef(false)
  const editing = useRef(false)
  const mounted = useRef(false)
  const generation = useRef(0)
  useEffect(() => {
    mounted.current = true
    let reading = false
    const refresh = async () => {
      if (reading || mutating.current || document.hidden) return
      reading = true
      const current = generation.current
      try {
        const [s, c] = await Promise.all([api.screen(), api.screenSettings()])
        if (mounted.current && current === generation.current) {
          setStatus(s); setSettings(c); setLoadError('')
          if (!editing.current) setDraft(c)
        }
      } catch (e) { if (mounted.current) setLoadError(e instanceof Error ? e.message : t('Unable to read DevUI settings')) }
      finally { reading = false }
    }
    void refresh()
    const timer = window.setInterval(() => void refresh(), 2000)
    return () => { mounted.current = false; window.clearInterval(timer) }
  }, [])
  function edit(patch: Partial<DevUISettings>) {
    editing.current = true; setMessage(''); setDraft(d => d ? { ...d, ...patch } : d)
  }
  async function save() {
    if (!draft) return
    mutating.current = true; setBusy(true); setError(''); setMessage(''); generation.current++
    try {
      const { revision, ...values } = draft
      const saved = await api.screenSettingsSet({ ...values, expected_revision: revision })
      if (mounted.current) { editing.current = false; setDraft(saved); setSettings(saved); setMessage(t('Saved. Display and menu changes apply within two seconds. The startup page applies next time DevUI opens.')) }
    } catch (e) { if (mounted.current) setError(e instanceof Error ? e.message : t('Unable to save DevUI settings')) }
    finally { generation.current++; mutating.current = false; if (mounted.current) setBusy(false) }
  }
  async function reload() {
    if (editing.current && !(await confirm({ title: t('Discard unsaved DevUI settings?'), body: t('Reload the settings saved on the device.'), confirmLabel: t('Reload') }))) return
    mutating.current = true; setBusy(true); generation.current++; setError('')
    try { const c = await api.screenSettings(); if (mounted.current) { editing.current = false; setDraft(c); setSettings(c); setMessage(t('Settings reloaded.')) } }
    catch (e) { if (mounted.current) setError(e instanceof Error ? e.message : t('Unable to reload settings')) }
    finally { generation.current++; mutating.current = false; if (mounted.current) setBusy(false) }
  }
  async function change(open: boolean) {
    mutating.current = true; setBusy(true); setError(''); generation.current++
    try {
      await (open ? api.screenOpen() : api.screenClose())
      if (mounted.current) setStatus(s => s ? { ...s, running: true, state: open ? 'starting' : 'restoring' } : s)
    } catch (e) { if (mounted.current) setError(e instanceof Error ? e.message : t('Screen action failed')) }
    finally { generation.current++; mutating.current = false; if (mounted.current) setBusy(false) }
  }
  const stale = draft && settings && draft.revision !== settings.revision
  return <div className="space-y-4">
    <PageHeader title="DevUI" sub={t('Device screen, ways to open it and live menu settings')} />
    {(error || loadError) && <p role="alert" className="rounded-lg border border-danger/20 bg-danger/5 p-3 text-sm text-danger">{tm(error || loadError)}</p>}
    {message && <p role="status" className="rounded-lg bg-accent/5 p-3 text-sm text-ink">{message}</p>}
    <Card title={t('Device screen')} action={<Chip tone={status?.state === 'error' ? 'warn' : status?.running ? 'accent' : 'default'}>{status ? t(labels[status.state] ?? status.state) : t('Loading')}</Chip>}>
      <div className="space-y-3">
        <p className="text-[13px] text-ink2">{t('Open the touchscreen menu on your U60 Pro. Short-press the power button to sleep or wake the display; hold it to shut down or restart. Returning to the stock UI keeps your proxy settings and network connection.')}</p>
        {status && (!status.available || !status.verified) && <p className="text-xs text-ink2">{t('The screen component must be installed and pass device recovery verification before it can open.')}</p>}
        <div className="flex flex-wrap gap-2">
          <Button variant="primary" disabled={busy || !status?.available || !status.verified || status.running} onClick={() => void change(true)}>{t('Open DevUI screen')}</Button>
          <Button variant="outline" disabled={busy || !status?.available || (!status.running && status.state !== 'error')} onClick={() => void change(false)}>{t('Restore stock UI')}</Button>
        </div>
      </div>
    </Card>
    <Card title={t('Ways to open DevUI')}>
      <div className="divide-y divide-line/10">
        <div className="flex items-start justify-between gap-4 pb-3"><div><p className="text-[13px] font-medium text-ink">{t('Hold two fingers to open DevUI')}</p><p className="mt-1 text-xs leading-relaxed text-ink2">{t('On the lit stock UI, hold two fingers still on the screen for 2 seconds. Put them where the stock UI has no button.')}</p><p className="mt-1 text-xs text-ink3">{status?.touch?.available ? tm(status.touch.message) : t('Touchscreen listener is not available on this device.')}</p></div><Toggle label={t('Enable two-finger hold')} checked={draft?.touch_enabled ?? false} disabled={busy || !draft} onChange={v => edit({ touch_enabled: v })} /></div>
        <div className="flex items-start justify-between gap-4 py-3"><div><p className="text-[13px] font-medium text-ink">{t('Open DevUI at startup')}</p><p className="mt-1 text-xs leading-relaxed text-ink2">{t('After the device starts, DevUI opens once the stock UI has finished starting. It does not open while the device charges switched off.')}</p></div><Toggle label={t('Enable opening at startup')} checked={draft?.open_at_boot ?? false} disabled={busy || !draft} onChange={v => edit({ open_at_boot: v })} /></div>
        <div className="flex items-start justify-between gap-4 pt-3"><div><p className="text-[13px] font-medium text-ink">{t('Press four times to open DevUI')}</p><p className="mt-1 text-xs leading-relaxed text-ink2">{t('From the stock UI, make four short presses within 2.5 seconds, with no more than 0.7 seconds between presses. Long presses keep their factory behavior.')}</p><p className="mt-1 text-xs text-ink3">{status?.shortcut?.available ? tm(status.shortcut.message) : t('Power-key listener is not available on this device.')} {t('The stock display may turn on and off as you press.')}</p></div><Toggle label={t('Enable four-press shortcut')} checked={draft?.shortcut_enabled ?? false} disabled={busy || !draft} onChange={v => edit({ shortcut_enabled: v })} /></div>
      </div>
    </Card>
    <Card title={t('Display preferences')}>
      {draft ? <div className="grid gap-5 sm:grid-cols-2">
        <label className="block text-[13px] text-ink">{t('Brightness')} <span className="float-right font-mono">{Math.round(draft.brightness / 255 * 100)}%</span><input aria-label={t('DevUI brightness')} className="mt-3 w-full accent-accent" type="range" min="20" max="255" value={draft.brightness} disabled={busy} onChange={e => edit({ brightness: Number(e.target.value) })} /><span className="mt-1 block text-xs text-ink3">{t('Applied live; changing brightness does not wake a sleeping screen.')}</span></label>
        <label className="block text-[13px] text-ink">{t('Idle time')}<Select aria-label={t('DevUI idle timeout')} className="mt-2 w-full" value={draft.idle_seconds} disabled={busy} onChange={e => edit({ idle_seconds: Number(e.target.value) })}>{Array.from(new Set([30, 60, 120, 300, 600, draft.idle_seconds])).sort((a, b) => a - b).map(n => <option key={n} value={n}>{n < 60 ? t('{n} seconds', { n }) : n === 60 ? t('{n} minute', { n: 1 }) : t('{n} minutes', { n: n / 60 })}</option>)}</Select></label>
        <label className="block text-[13px] text-ink">{t('When idle')}<Select aria-label={t('DevUI idle action')} className="mt-2 w-full" value={draft.idle_sleep ? 'sleep' : 'stock'} disabled={busy} onChange={e => edit({ idle_sleep: e.target.value === 'sleep' })}><option value="sleep">{t('Turn the screen off, stay in DevUI')}</option><option value="stock">{t('Return to the stock UI')}</option></Select><span className="mt-2 block text-xs text-ink3">{draft.idle_sleep ? t('A short power press wakes the screen.') : t('Unsaved device routing choices are discarded on idle return.')}</span></label>
        <label className="block text-[13px] text-ink">{t('Startup page')}<Select aria-label={t('DevUI startup page')} className="mt-2 w-full" value={draft.home_page} disabled={busy} onChange={e => edit({ home_page: e.target.value as DevUISettings['home_page'] })}><option value="menu">{t('Main menu')}</option>{pages.filter(p => draft.menu_items.includes(p.id)).map(p => <option key={p.id} value={p.id}>{t(p.title)}</option>)}</Select></label>
        <div className="block text-[13px] text-ink">{t('Screen language')}<span className="float-right">{LANGUAGES[draft.language ?? 'zh']}</span><span className="mt-2 block text-xs text-ink3">{t('Follows the language switch of this panel.')}</span></div>
      </div> : <p className="text-sm text-ink2">{t('Loading display preferences…')}</p>}
    </Card>
    <Card title={t('Menu features')} action={<div className="flex items-center gap-1.5"><Button variant="outline" disabled={busy || !draft} onClick={() => edit({ menu_items: GRID_PRESET, home_page: 'menu' })}>{t('3×3 grid preset')}</Button><Button variant="outline" disabled={busy || !draft} onClick={() => edit({ menu_items: CLASSIC_PRESET, home_page: 'menu' })}>{t('Classic 4-row preset')}</Button></div>}>
      <p className="mb-3 text-xs text-ink2">{t('Choose the controls shown on the device. Hiding a menu item does not disable its network feature. Stock UI and Back remain available.')} {t('Up to 9 features fit the 3×3 grid (1–4 items use rows). Saved profiles and Device routing stay reachable from the WireGuard page.')}</p>
      <div className="divide-y divide-line/10">{pages.map(p => <label key={p.id} className="flex cursor-pointer items-center gap-3 py-3"><input type="checkbox" className="h-4 w-4 accent-accent" checked={draft?.menu_items.includes(p.id) ?? false} disabled={busy || !draft || (!draft.menu_items.includes(p.id) && draft.menu_items.length >= MAX_MENU_ITEMS)} onChange={e => { if (!draft) return; const items = e.target.checked ? [...draft.menu_items, p.id] : draft.menu_items.filter(id => id !== p.id); edit({ menu_items: items, home_page: draft.home_page !== 'menu' && !items.includes(draft.home_page) ? 'menu' : draft.home_page }) }} /><span><span className="block text-[13px] font-medium text-ink">{t(p.title)}</span><span className="text-xs text-ink2">{t(p.description)}</span></span></label>)}</div>
    </Card>
    <div className="sticky bottom-2 rounded-xl border border-line/10 bg-surface p-3 shadow-sm">
      {stale && <p role="alert" className="mb-2 text-xs text-danger">{t('Settings changed in another session. Reload before saving.')}</p>}
      {draft?.menu_items.length === 0 && <p role="alert" className="mb-2 text-xs text-danger">{t('Keep at least one menu feature enabled.')}</p>}
      <div className="flex flex-wrap items-center gap-2"><Button variant="primary" disabled={busy || !editing.current || !draft?.menu_items.length || !!stale} onClick={() => void save()}>{busy ? t('Working…') : t('Save DevUI settings')}</Button><Button variant="outline" disabled={busy} onClick={() => void reload()}>{t('Reload settings')}</Button><span className="text-xs text-ink3">{editing.current ? t('Unsaved changes') : t('Settings saved on device')}</span></div>
    </div>
  </div>
}
