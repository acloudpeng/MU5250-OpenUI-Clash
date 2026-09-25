import { useState } from 'react'
import { api } from '../../data/api'
import { formatBytes } from '../../format'
import type { ClashSubscription, ClashSubscriptionAction } from '../../types'
import { Button, Field, Input } from '../../ui/controls'
import { confirm, toast } from '../../ui/feedback'
import { Card, Chip, Meter } from '../../ui/primitives'
import { useClash } from './ClashContext'
import { locale, t, tm } from '../../i18n'

interface Draft {
  id: number | null
  url: string
  name: string
  user_agent: string
  interval_hours: string
  via_proxy: boolean
}

const EMPTY: Draft = { id: null, url: '', name: '', user_agent: '', interval_hours: '', via_proxy: false }

function ago(ts: number) {
  if (!ts) return t('never')
  const s = Math.max(0, Math.round(Date.now() / 1000 - ts))
  return s < 60 ? t('just now') : s < 3600 ? t('{n} min ago', { n: Math.round(s / 60) }) : s < 86400 ? t('{n} h ago', { n: Math.round(s / 3600) }) : t('{n} d ago', { n: Math.round(s / 86400) })
}

function daysLeft(expire: number) {
  return expire ? Math.ceil((expire - Date.now() / 1000) / 86400) : null
}

function Usage({ sub }: { sub: ClashSubscription }) {
  const u = sub.usage
  if (!u) return null
  const used = u.upload + u.download
  const days = daysLeft(u.expire)
  return (
    <div className="space-y-1">
      {u.total > 0 && <Meter pct={(used / u.total) * 100} tone={used / u.total > 0.9 ? 'bg-danger' : 'bg-accent'} />}
      <p className="text-[11px] text-ink3">
        {u.total > 0 ? t('{used} of {total}', { used: formatBytes(used), total: formatBytes(u.total) }) : t('{used} used', { used: formatBytes(used) })}
        {days !== null && (days > 0 ? ` · ${t('expires in {days} d ({date})', { days, date: new Date(u.expire * 1000).toLocaleDateString(locale()) })}` : ` · ${t('expired')}`)}
      </p>
    </div>
  )
}

export default function SubscriptionsTab() {
  const { status, busy, update, run } = useClash()
  const [draft, setDraft] = useState<Draft | null>(null)
  const subs = status?.subscriptions ?? []
  const max = status?.max_subscriptions ?? 5

  async function act(action: ClashSubscriptionAction, done: string, failure: string) {
    const s = await update({ subscription: action }, failure)
    if (s) toast(done)
    return s
  }
  async function activate(sub: ClashSubscription) {
    if (status?.enabled && !(await confirm({ title: t('Switch to {name}?', { name: sub.name }), body: t('The core reloads with this profile. Proxy traffic pauses briefly and open connections restart.'), confirmLabel: t('Switch') }))) return
    await act({ action: 'activate', id: sub.id }, status?.enabled ? t('Switched subscription.') : t('Subscription selected. Clash remains off.'), t('Unable to switch subscription'))
  }
  async function remove(sub: ClashSubscription) {
    if (!(await confirm({ title: t('Delete {name}?', { name: sub.name }), body: t('The saved URL and downloaded profile are removed.'), confirmLabel: t('Delete'), danger: true }))) return
    await act({ action: 'delete', id: sub.id }, t('Subscription deleted.'), t('Unable to delete subscription'))
  }
  async function refresh(sub: ClashSubscription) {
    const s = await run(() => api.clashRefresh(sub.id), t('Subscription update failed'))
    if (s) toast(t('{name} updated.', { name: sub.name }))
  }
  async function submit(d: Draft) {
    const hours = d.interval_hours.trim() === '' ? undefined : Number(d.interval_hours)
    if (hours !== undefined && (!Number.isInteger(hours) || hours < 0 || hours > 720)) { toast(t('Update interval must be 0–720 hours'), 'err'); return }
    const common = { name: d.name.trim() || undefined, user_agent: d.user_agent.trim() || undefined, interval_hours: hours, via_proxy: d.via_proxy }
    const s = d.id === null
      ? await act({ action: 'add', url: d.url.trim(), ...common }, t('Subscription downloaded and saved.'), t('Unable to add subscription'))
      : await act({ action: 'edit', id: d.id, ...common, ...(d.url.trim() ? { url: d.url.trim() } : {}) }, t('Subscription saved.'), t('Unable to save subscription'))
    if (s) setDraft(null)
  }

  return (
    <div className="space-y-4">
      <Card title={t('Subscriptions')} action={<Chip>{subs.length} / {max}</Chip>}>
        <div className="space-y-3">
          {!status && <p className="text-sm text-ink2">{t('Loading…')}</p>}
          {status && subs.length === 0 && <p className="text-sm text-ink2">{t('No subscriptions yet. Add your provider’s Clash subscription URL below.')}</p>}
          {subs.map(sub => {
            const current = sub.id === status?.active_subscription
            return (
              <div key={sub.id} className={`space-y-2 rounded-lg border p-3 ${current ? 'border-accent/40 bg-accent/5' : 'border-line/10'}`}>
                <div className="flex flex-wrap items-center gap-2">
                  <span className="min-w-0 flex-1 truncate text-[13px] font-semibold text-ink">{sub.name}</span>
                  {current && <Chip tone="accent">{t('In use')}</Chip>}
                  {!sub.downloaded && <Chip tone="warn">{t('Not downloaded')}</Chip>}
                  {sub.via_proxy && <Chip>{t('Via proxy')}</Chip>}
                </div>
                <p className="break-all font-mono text-[11px] text-ink3">{sub.url}</p>
                <p className="text-xs text-ink2">
                  {t('{n} nodes', { n: sub.proxies })} · {t('{n} groups', { n: sub.groups })} · {t('{n} rules', { n: sub.rules })}{sub.providers ? ` · ${t('{n} providers', { n: sub.providers })}` : ''} · {sub.updated_at ? t('updated {time}', { time: ago(sub.updated_at) }) : t('never updated')}
                  {sub.interval_hours > 0 ? ` · ${t('auto every {n} h', { n: sub.interval_hours })}` : ` · ${t('manual updates')}`}
                </p>
                <Usage sub={sub} />
                {sub.error && <p role="alert" className="rounded-md bg-danger/10 px-2 py-1.5 text-xs text-danger">{tm(sub.error)}</p>}
                <div className="flex flex-wrap gap-2">
                  <Button size="sm" variant="primary" disabled={busy || current} onClick={() => void activate(sub)}>{status?.enabled ? t('Switch & apply') : t('Use')}</Button>
                  <Button size="sm" variant="outline" disabled={busy} onClick={() => void refresh(sub)}>{t('Update now')}</Button>
                  <Button size="sm" variant="outline" disabled={busy} onClick={() => setDraft({ id: sub.id, url: '', name: sub.name, user_agent: sub.user_agent, interval_hours: String(sub.interval_hours), via_proxy: sub.via_proxy })}>{t('Edit')}</Button>
                  <Button size="sm" variant="danger" disabled={busy || (current && !!status?.enabled)} onClick={() => void remove(sub)}>{t('Delete')}</Button>
                </div>
              </div>
            )
          })}
          {status && subs.length < max && draft === null && <Button variant="outline" disabled={busy} onClick={() => setDraft({ ...EMPTY })}>{t('Add subscription')}</Button>}
          {status && subs.length >= max && <p className="text-xs text-ink2">{t('All {max} slots are in use. Delete a subscription before adding another.', { max })}</p>}
        </div>
      </Card>

      {draft && <Card title={draft.id === null ? t('Add subscription') : t('Edit subscription')}>
        <div className="space-y-3">
          <Field label={draft.id === null ? t('Subscription URL') : t('New URL (leave empty to keep)')} hint={t('Must return a complete Clash / mihomo YAML profile. Treated as a secret: only the host is shown afterwards.')}>
            <Input aria-label={t('Subscription URL')} type="url" autoComplete="off" spellCheck={false} value={draft.url} disabled={busy} placeholder="https://provider.example/api/v1/client/subscribe?token=…" onChange={e => setDraft({ ...draft, url: e.target.value })} />
          </Field>
          <div className="grid gap-3 sm:grid-cols-2">
            <Field label={t('Name')} hint={draft.id === null ? t('Optional; defaults to the provider’s name.') : undefined}>
              <Input aria-label={t('Subscription name')} value={draft.name} maxLength={48} disabled={busy} onChange={e => setDraft({ ...draft, name: e.target.value })} />
            </Field>
            <Field label={t('Update every (hours)')} hint={t('0 disables automatic updates. Empty uses the provider’s interval or 24 h.')}>
              <Input aria-label={t('Update interval in hours')} inputMode="numeric" value={draft.interval_hours} disabled={busy} onChange={e => setDraft({ ...draft, interval_hours: e.target.value.replace(/[^0-9]/g, '') })} />
            </Field>
            <Field label={t('User-Agent')} hint={t('Default clash.meta. Some providers pick the profile format from it.')}>
              <Input aria-label={t('Subscription user agent')} value={draft.user_agent} maxLength={128} disabled={busy} placeholder="clash.meta" onChange={e => setDraft({ ...draft, user_agent: e.target.value })} />
            </Field>
          </div>
          <label className="flex items-center gap-2 text-[13px] text-ink">
            <input type="checkbox" className="h-4 w-4 accent-accent" checked={draft.via_proxy} disabled={busy} onChange={e => setDraft({ ...draft, via_proxy: e.target.checked })} />
            {t('Download through the running proxy (when the provider is blocked locally)')}
          </label>
          <div className="flex flex-wrap gap-2">
            <Button variant="primary" loading={busy} disabled={draft.id === null && !draft.url.trim()} onClick={() => void submit(draft)}>{draft.id === null ? t('Download & save') : t('Save')}</Button>
            <Button variant="ghost" disabled={busy} onClick={() => setDraft(null)}>{t('Cancel')}</Button>
          </div>
          <p className="text-xs text-ink3">{t('The profile is checked by the core before it is used; a broken download keeps the last working profile. Base64 node lists are not supported — ask your provider for the Clash format.')}</p>
        </div>
      </Card>}
    </div>
  )
}
