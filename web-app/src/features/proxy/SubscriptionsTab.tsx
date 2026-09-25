import { useState } from 'react'
import { api } from '../../data/api'
import { formatBytes } from '../../format'
import type { ClashSubscription, ClashSubscriptionAction } from '../../types'
import { Button, Field, Input } from '../../ui/controls'
import { confirm, toast } from '../../ui/feedback'
import { Card, Chip, Meter } from '../../ui/primitives'
import { useClash } from './ClashContext'

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
  if (!ts) return 'never'
  const s = Math.max(0, Math.round(Date.now() / 1000 - ts))
  return s < 60 ? 'just now' : s < 3600 ? `${Math.round(s / 60)} min ago` : s < 86400 ? `${Math.round(s / 3600)} h ago` : `${Math.round(s / 86400)} d ago`
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
        {formatBytes(used)}{u.total > 0 ? ` of ${formatBytes(u.total)}` : ' used'}
        {days !== null && (days > 0 ? ` · expires in ${days} d (${new Date(u.expire * 1000).toLocaleDateString()})` : ' · expired')}
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
    if (status?.enabled && !(await confirm({ title: `Switch to ${sub.name}?`, body: 'The core reloads with this profile. Proxy traffic pauses briefly and open connections restart.', confirmLabel: 'Switch' }))) return
    await act({ action: 'activate', id: sub.id }, status?.enabled ? 'Switched subscription.' : 'Subscription selected. Clash remains off.', 'Unable to switch subscription')
  }
  async function remove(sub: ClashSubscription) {
    if (!(await confirm({ title: `Delete ${sub.name}?`, body: 'The saved URL and downloaded profile are removed.', confirmLabel: 'Delete', danger: true }))) return
    await act({ action: 'delete', id: sub.id }, 'Subscription deleted.', 'Unable to delete subscription')
  }
  async function refresh(sub: ClashSubscription) {
    const s = await run(() => api.clashRefresh(sub.id), 'Subscription update failed')
    if (s) toast(`${sub.name} updated.`)
  }
  async function submit(d: Draft) {
    const hours = d.interval_hours.trim() === '' ? undefined : Number(d.interval_hours)
    if (hours !== undefined && (!Number.isInteger(hours) || hours < 0 || hours > 720)) { toast('Update interval must be 0–720 hours', 'err'); return }
    const common = { name: d.name.trim() || undefined, user_agent: d.user_agent.trim() || undefined, interval_hours: hours, via_proxy: d.via_proxy }
    const s = d.id === null
      ? await act({ action: 'add', url: d.url.trim(), ...common }, 'Subscription downloaded and saved.', 'Unable to add subscription')
      : await act({ action: 'edit', id: d.id, ...common, ...(d.url.trim() ? { url: d.url.trim() } : {}) }, 'Subscription saved.', 'Unable to save subscription')
    if (s) setDraft(null)
  }

  return (
    <div className="space-y-4">
      <Card title="Subscriptions" action={<Chip>{subs.length} / {max}</Chip>}>
        <div className="space-y-3">
          {!status && <p className="text-sm text-ink2">Loading…</p>}
          {status && subs.length === 0 && <p className="text-sm text-ink2">No subscriptions yet. Add your provider’s Clash subscription URL below.</p>}
          {subs.map(sub => {
            const current = sub.id === status?.active_subscription
            return (
              <div key={sub.id} className={`space-y-2 rounded-lg border p-3 ${current ? 'border-accent/40 bg-accent/5' : 'border-line/10'}`}>
                <div className="flex flex-wrap items-center gap-2">
                  <span className="min-w-0 flex-1 truncate text-[13px] font-semibold text-ink">{sub.name}</span>
                  {current && <Chip tone="accent">In use</Chip>}
                  {!sub.downloaded && <Chip tone="warn">Not downloaded</Chip>}
                  {sub.via_proxy && <Chip>Via proxy</Chip>}
                </div>
                <p className="break-all font-mono text-[11px] text-ink3">{sub.url}</p>
                <p className="text-xs text-ink2">
                  {sub.proxies} nodes · {sub.groups} groups · {sub.rules} rules{sub.providers ? ` · ${sub.providers} providers` : ''} · updated {ago(sub.updated_at)}
                  {sub.interval_hours > 0 ? ` · auto every ${sub.interval_hours} h` : ' · manual updates'}
                </p>
                <Usage sub={sub} />
                {sub.error && <p role="alert" className="rounded-md bg-danger/10 px-2 py-1.5 text-xs text-danger">{sub.error}</p>}
                <div className="flex flex-wrap gap-2">
                  <Button size="sm" variant="primary" disabled={busy || current} onClick={() => void activate(sub)}>{status?.enabled ? 'Switch & apply' : 'Use'}</Button>
                  <Button size="sm" variant="outline" disabled={busy} onClick={() => void refresh(sub)}>Update now</Button>
                  <Button size="sm" variant="outline" disabled={busy} onClick={() => setDraft({ id: sub.id, url: '', name: sub.name, user_agent: sub.user_agent, interval_hours: String(sub.interval_hours), via_proxy: sub.via_proxy })}>Edit</Button>
                  <Button size="sm" variant="danger" disabled={busy || (current && !!status?.enabled)} onClick={() => void remove(sub)}>Delete</Button>
                </div>
              </div>
            )
          })}
          {status && subs.length < max && draft === null && <Button variant="outline" disabled={busy} onClick={() => setDraft({ ...EMPTY })}>Add subscription</Button>}
          {status && subs.length >= max && <p className="text-xs text-ink2">All {max} slots are in use. Delete a subscription before adding another.</p>}
        </div>
      </Card>

      {draft && <Card title={draft.id === null ? 'Add subscription' : 'Edit subscription'}>
        <div className="space-y-3">
          <Field label={draft.id === null ? 'Subscription URL' : 'New URL (leave empty to keep)'} hint="Must return a complete Clash / mihomo YAML profile. Treated as a secret: only the host is shown afterwards.">
            <Input aria-label="Subscription URL" type="url" autoComplete="off" spellCheck={false} value={draft.url} disabled={busy} placeholder="https://provider.example/api/v1/client/subscribe?token=…" onChange={e => setDraft({ ...draft, url: e.target.value })} />
          </Field>
          <div className="grid gap-3 sm:grid-cols-2">
            <Field label="Name" hint={draft.id === null ? 'Optional; defaults to the provider’s name.' : undefined}>
              <Input aria-label="Subscription name" value={draft.name} maxLength={48} disabled={busy} onChange={e => setDraft({ ...draft, name: e.target.value })} />
            </Field>
            <Field label="Update every (hours)" hint="0 disables automatic updates. Empty uses the provider’s interval or 24 h.">
              <Input aria-label="Update interval in hours" inputMode="numeric" value={draft.interval_hours} disabled={busy} onChange={e => setDraft({ ...draft, interval_hours: e.target.value.replace(/[^0-9]/g, '') })} />
            </Field>
            <Field label="User-Agent" hint="Default clash.meta. Some providers pick the profile format from it.">
              <Input aria-label="Subscription user agent" value={draft.user_agent} maxLength={128} disabled={busy} placeholder="clash.meta" onChange={e => setDraft({ ...draft, user_agent: e.target.value })} />
            </Field>
          </div>
          <label className="flex items-center gap-2 text-[13px] text-ink">
            <input type="checkbox" className="h-4 w-4 accent-accent" checked={draft.via_proxy} disabled={busy} onChange={e => setDraft({ ...draft, via_proxy: e.target.checked })} />
            Download through the running proxy (when the provider is blocked locally)
          </label>
          <div className="flex flex-wrap gap-2">
            <Button variant="primary" loading={busy} disabled={draft.id === null && !draft.url.trim()} onClick={() => void submit(draft)}>{draft.id === null ? 'Download & save' : 'Save'}</Button>
            <Button variant="ghost" disabled={busy} onClick={() => setDraft(null)}>Cancel</Button>
          </div>
          <p className="text-xs text-ink3">The profile is checked by the core before it is used; a broken download keeps the last working profile. Base64 node lists are not supported — ask your provider for the Clash format.</p>
        </div>
      </Card>}
    </div>
  )
}
