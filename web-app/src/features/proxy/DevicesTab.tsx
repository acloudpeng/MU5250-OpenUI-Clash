import { useState } from 'react'
import { api } from '../../data/api'
import { usePoll } from '../../data/poll'
import type { Client } from '../../types'
import { Button, Select } from '../../ui/controls'
import { toast } from '../../ui/feedback'
import { Card, Chip } from '../../ui/primitives'
import { useClash } from './ClashContext'

type Mode = 'all' | 'selected'

export default function DevicesTab() {
  const { status, busy, update } = useClash()
  const { data: clients } = usePoll('clash-clients', api.clients, 15000)
  // Unsaved edits and the revision they were made against; null follows the device.
  const [draft, setDraft] = useState<{ mode: Mode; macs: string[]; revision: number } | null>(null)
  const mode = draft?.mode ?? status?.mode ?? 'all'
  const macs = draft?.macs ?? status?.macs ?? []

  const known = new Map((clients ?? []).filter(c => c.mac).map(c => [c.mac.toLowerCase(), c]))
  for (const m of macs) if (!known.has(m)) known.set(m, { mac: m, hostname: 'Saved device (offline)' } as Client)
  const rows = [...known.values()]
  const selected = (m: string) => mode === 'all' ? !macs.includes(m) : macs.includes(m)
  const dirty = !!status && !!draft && (mode !== status.mode || [...macs].sort().join() !== [...status.macs].sort().join())
  const change = (next: Mode, nextMacs: string[]) => { if (status) setDraft({ mode: next, macs: nextMacs, revision: draft?.revision ?? status.revision }) }

  async function save() {
    if (!draft) return
    const s = await update({ mode: draft.mode, macs: draft.macs }, 'Unable to save device choices', draft.revision)
    if (s) { setDraft(null); toast(s.enabled ? 'Device routing applied.' : 'Device choices saved. Clash remains off.') }
  }

  return (
    <Card title="Which devices use Clash?" action={dirty ? <Chip tone="warn">Unsaved changes</Chip> : undefined}>
      <div className="space-y-3">
        <Select aria-label="Device routing mode" value={mode} disabled={busy || !status} onChange={e => { const next = e.target.value as Mode; change(next, rows.filter(c => next === 'selected' ? selected(c.mac.toLowerCase()) : !selected(c.mac.toLowerCase())).map(c => c.mac.toLowerCase())) }}>
          <option value="all">All devices — new devices join automatically</option>
          <option value="selected">Selected devices only — new devices use normal network</option>
        </Select>
        <p className="text-xs text-ink2">Checked: through Clash. Unchecked: normal network. Choices follow the device MAC address; a private/random MAC counts as a different device. These choices are separate from WireGuard’s.</p>
        <div className="divide-y divide-line/10 rounded-lg border border-line/10">
          {rows.length === 0 ? <p className="p-4 text-sm text-ink3">No devices found. In all-devices mode, new connections will use Clash.</p> : rows.map(c => {
            const m = c.mac.toLowerCase(), checked = selected(m)
            return <label key={m} className="flex cursor-pointer items-center gap-3 px-3 py-3 hover:bg-surface2/50">
              <input type="checkbox" className="h-4 w-4 shrink-0 accent-accent" checked={checked} disabled={busy || !status} onChange={() => change(mode, macs.includes(m) ? macs.filter(v => v !== m) : [...macs, m])} />
              <span className="min-w-0 flex-1"><span className="block truncate text-[13px] font-medium text-ink">{c.hostname || c.ip || 'Connected device'}</span><span className="block break-all font-mono text-[11px] text-ink3">{c.ip || 'Offline'} · {m}</span></span>
              <Chip tone={checked ? 'accent' : 'default'}>{checked ? 'Clash' : 'Direct'}</Chip>
            </label>
          })}
        </div>
        <p className="rounded-lg bg-surface2/60 p-3 text-xs leading-relaxed text-ink2">Proxy devices cannot fall back to the normal network if the core stops. Their DNS (any server, UDP and TCP) is answered by the core. Router management stays accessible. After changing a device’s route, reconnect its apps or Wi-Fi so existing connections restart.</p>
        <div className="flex flex-wrap gap-2">
          <Button variant="primary" loading={busy} disabled={!status || !dirty} onClick={() => void save()}>Save {status?.enabled ? '& apply' : 'device choices'}</Button>
          <Button variant="ghost" disabled={busy || !draft} onClick={() => setDraft(null)}>Discard</Button>
        </div>
      </div>
    </Card>
  )
}
