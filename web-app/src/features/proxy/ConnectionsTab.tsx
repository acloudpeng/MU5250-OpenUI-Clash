import { useState } from 'react'
import { api } from '../../data/api'
import { usePoll } from '../../data/poll'
import { formatBytes } from '../../format'
import { Button, Input } from '../../ui/controls'
import { confirm, toast, toastError } from '../../ui/feedback'
import { Card, Chip, Stat } from '../../ui/primitives'
import { useClash } from './ClashContext'

function since(start: string) {
  const s = Math.max(0, Math.round((Date.now() - Date.parse(start)) / 1000))
  return Number.isFinite(s) ? (s < 60 ? `${s}s` : s < 3600 ? `${Math.floor(s / 60)}m` : `${Math.floor(s / 3600)}h`) : ''
}

export default function ConnectionsTab() {
  const { status } = useClash()
  const running = !!status?.running
  const [filter, setFilter] = useState('')
  const [paused, setPaused] = useState(false)
  const [closing, setClosing] = useState(false)
  const { data, error, refresh } = usePoll('clash-connections', api.clashConnections, 3000, running && !paused)

  async function closeAll() {
    if (!(await confirm({ title: 'Close all connections?', body: 'Apps reconnect on their own, through the currently selected nodes.', confirmLabel: 'Close all', danger: true }))) return
    setClosing(true)
    try { await api.clashCloseConnections(); toast('Connections closed.'); refresh() }
    catch (e) { toastError(e, 'Unable to close connections') }
    finally { setClosing(false) }
  }

  if (!status) return <p className="text-sm text-ink2">Loading…</p>
  if (!running) return <Card><p className="text-sm text-ink2">Connections are shown while the Clash core is running.</p></Card>

  const needle = filter.trim().toLowerCase()
  const list = (data?.connections ?? [])
    .filter(c => !needle || [c.host, c.sniff_host, c.destination, c.source, c.rule, c.rule_payload, ...c.chains].some(v => v.toLowerCase().includes(needle)))
    .sort((a, b) => Date.parse(b.start) - Date.parse(a.start))

  return (
    <div className="space-y-4">
      <Card>
        <div className="grid grid-cols-3 gap-4">
          <Stat label="Active" value={data?.connections.length ?? '—'} />
          <Stat label="Downloaded" value={data ? formatBytes(data.download_total) : '—'} sub="since core start" />
          <Stat label="Uploaded" value={data ? formatBytes(data.upload_total) : '—'} sub="since core start" />
        </div>
      </Card>
      <Card title="Connections" action={<div className="flex gap-2">
        <Button size="sm" variant="ghost" onClick={() => setPaused(p => !p)}>{paused ? 'Resume' : 'Pause'}</Button>
        <Button size="sm" variant="danger" loading={closing} disabled={!data?.connections.length} onClick={() => void closeAll()}>Close all</Button>
      </div>}>
        <div className="space-y-3">
          <Input aria-label="Filter connections" placeholder="Filter by host, IP, rule or node" value={filter} onChange={e => setFilter(e.target.value)} />
          {error && <p role="alert" className="rounded-lg bg-danger/10 p-3 text-[13px] text-danger">{error}</p>}
          {data && list.length === 0 && <p className="text-sm text-ink3">{needle ? 'No matching connections.' : 'No open connections.'}</p>}
          <div className="divide-y divide-line/10">
            {list.map(c => (
              <div key={c.id} className="space-y-1 py-2">
                <div className="flex items-center gap-2">
                  <span className="min-w-0 flex-1 truncate text-[13px] font-medium text-ink">{c.host || c.sniff_host || c.destination}:{c.port}</span>
                  <Chip>{c.network.toUpperCase()}</Chip>
                </div>
                <p className="truncate text-[11px] text-ink2">{c.chains.length ? [...c.chains].reverse().join(' → ') : 'DIRECT'} · {c.rule}{c.rule_payload ? ` (${c.rule_payload})` : ''}</p>
                <p className="tnum text-[11px] text-ink3">{c.source} · ↓ {formatBytes(c.download)} ↑ {formatBytes(c.upload)} · {since(c.start)}</p>
              </div>
            ))}
          </div>
          {data && data.connections.length >= 500 && <p className="text-xs text-ink3">Showing the first 500 connections.</p>}
        </div>
      </Card>
    </div>
  )
}
