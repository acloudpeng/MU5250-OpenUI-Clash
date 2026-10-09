import { useEffect, useRef, useState } from 'react'
import { api } from '../../data/api'
import { usePoll } from '../../data/poll'
import type { ClashGroup, ClashProxies } from '../../types'
import { Button } from '../../ui/controls'
import { toastError } from '../../ui/feedback'
import { Card, Chip } from '../../ui/primitives'
import { useClash } from './ClashContext'
import { t, tm } from '../../i18n'

function delayTone(d: number | null | undefined) {
  if (!d) return 'text-ink3'
  return d < 200 ? 'text-ok' : d < 500 ? 'text-warn' : 'text-danger'
}

export default function ProxiesTab() {
  const { status } = useClash()
  const running = !!status?.running
  const { data, error, mutate } = usePoll('clash-proxies', api.clashProxies, 15000, running)
  // Delay results arrive after later polls; merge them into the newest view.
  const latest = useRef<ClashProxies | null>(null)
  useEffect(() => { latest.current = data }, [data])
  const [pending, setPending] = useState<string | null>(null)
  const [testing, setTesting] = useState<Set<string>>(new Set())
  const [showHidden, setShowHidden] = useState(false)
  const [open, setOpen] = useState<Set<string>>(new Set())
  const mounted = useRef(true)
  useEffect(() => { mounted.current = true; return () => { mounted.current = false } }, [])
  const setDelays = (delays: Record<string, number | null>) => {
    const prev = latest.current
    if (!prev) return
    const nodes = { ...prev.nodes }
    for (const [n, d] of Object.entries(delays)) if (nodes[n]) nodes[n] = { ...nodes[n], delay: d && d > 0 ? d : null, alive: !!d && d > 0 }
    mutate({ ...prev, nodes })
  }

  async function select(group: ClashGroup, name: string) {
    if (!group.selectable || group.now === name || pending) return
    setPending(group.name)
    try {
      mutate(await api.clashSelect(group.name, name))
    } catch (e) { toastError(e, t('Unable to switch node')) }
    finally { if (mounted.current) setPending(null) }
  }
  async function test(name: string, group: boolean) {
    setTesting(prev => new Set(prev).add(name))
    try {
      const result = await api.clashDelay(name, group)
      setDelays(group ? result : { [name]: result.delay ?? 0 })
    } catch (e) {
      // A single node that times out is a result, not an error.
      if (group) toastError(e, t('Delay test failed'))
      else setDelays({ [name]: null })
    }
    finally { if (mounted.current) setTesting(prev => { const next = new Set(prev); next.delete(name); return next }) }
  }

  if (!status) return <p className="text-sm text-ink2">{t('Loading…')}</p>
  if (!running) return <Card><p className="text-sm text-ink2">{t('Nodes are listed while the Clash core is running. Choose Clash under Active proxy.')}</p></Card>

  const groups = (data?.groups ?? []).filter(g => showHidden || !g.hidden)
  const hiddenCount = (data?.groups ?? []).filter(g => g.hidden).length

  return (
    <div className="space-y-4">
      {error && <p role="alert" className="rounded-lg bg-danger/10 p-3 text-[13px] text-danger">{tm(error)}</p>}
      {status.proxy_mode !== 'rule' && <p className="rounded-lg bg-surface2/60 p-3 text-xs text-ink2">
        {status.proxy_mode === 'global' ? t('Global mode: only the GLOBAL group decides where traffic goes.') : t('Direct mode: node choices are kept but not used until you switch back.')}
      </p>}
      {!data && !error && <p className="text-sm text-ink2">{t('Loading nodes…')}</p>}
      {groups.map(g => {
        const expanded = open.has(g.name)
        const nowNode = g.now ? data?.nodes[g.now] : undefined
        return (
          <Card key={g.name} title={<span className="flex items-center gap-2">{g.name}<span className="text-[11px] font-normal text-ink3">{g.type}</span></span>}
            action={<Button size="sm" variant="ghost" loading={testing.has(g.name)} onClick={() => void test(g.name, true)}>{t('Test')}</Button>}>
            <button type="button" className="flex w-full items-center justify-between gap-2 text-left" onClick={() => setOpen(prev => { const next = new Set(prev); if (next.has(g.name)) next.delete(g.name); else next.add(g.name); return next })}>
              <span className="min-w-0 truncate text-[13px] text-ink">{g.now ?? '—'}</span>
              <span className="flex shrink-0 items-center gap-2">
                <span className={`tnum text-xs ${delayTone(nowNode?.delay)}`}>{nowNode?.delay ? `${nowNode.delay} ms` : ''}</span>
                <span className="text-xs text-ink3">{g.all.length} · {expanded ? t('Hide') : t('Show')}</span>
              </span>
            </button>
            {expanded && <div className="mt-3 grid grid-cols-1 gap-2 sm:grid-cols-2 lg:grid-cols-3">
              {g.all.map(n => {
                const node = data?.nodes[n]
                const chosen = g.now === n
                return (
                  <div key={n} className={`flex items-center gap-2 rounded-lg border px-3 py-2 ${chosen ? 'border-accent/50 bg-accent/5' : 'border-line/10'} ${g.selectable ? 'cursor-pointer hover:bg-surface2/50' : ''}`}
                    role={g.selectable ? 'button' : undefined} tabIndex={g.selectable ? 0 : undefined} aria-pressed={g.selectable ? chosen : undefined}
                    onClick={() => void select(g, n)} onKeyDown={e => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); void select(g, n) } }}>
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-[13px] font-medium text-ink">{n}</span>
                      <span className="block text-[11px] text-ink3">{node?.type ?? ''}{node?.udp ? ' · UDP' : ''}</span>
                    </span>
                    <button type="button" className={`tnum shrink-0 rounded px-1.5 py-0.5 text-xs hover:bg-surface2 ${delayTone(node?.delay)}`} disabled={testing.has(n)}
                      onClick={e => { e.stopPropagation(); void test(n, false) }} aria-label={t('Test {node}', { node: n })}>
                      {testing.has(n) ? '…' : node?.delay ? `${node.delay} ms` : node?.alive === false ? t('Timeout') : t('Test')}
                    </button>
                  </div>
                )
              })}
            </div>}
            {pending === g.name && <p className="mt-2 text-xs text-ink3">{t('Switching…')}</p>}
            {!g.selectable && expanded && <p className="mt-2 text-xs text-ink3">{t('This group picks its node automatically.')}</p>}
          </Card>
        )
      })}
      {hiddenCount > 0 && <Button variant="ghost" size="sm" onClick={() => setShowHidden(v => !v)}>{showHidden ? hiddenCount > 1 ? t('Hide {n} hidden groups', { n: hiddenCount }) : t('Hide {n} hidden group', { n: hiddenCount }) : hiddenCount > 1 ? t('Show {n} hidden groups', { n: hiddenCount }) : t('Show {n} hidden group', { n: hiddenCount })}</Button>}
      {data && <div className="flex flex-wrap gap-2"><Chip>{t('{n} proxies', { n: Object.keys(data.nodes).length })}</Chip><Chip>{t('{n} groups', { n: data.groups.length })}</Chip></div>}
    </div>
  )
}
