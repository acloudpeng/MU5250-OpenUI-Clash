/* eslint-disable react-refresh/only-export-components */
import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from 'react'
import { api } from '../../data/api'
import { ApiError } from '../../data/client'
import type { ProxyBackendId, ProxyChoice, ProxyStatus } from '../../types'
import { confirm, toast, toastError } from '../../ui/feedback'
import { t } from '../../i18n'

// The active proxy backend (agent/src/proxy.rs). The Clash and WireGuard views keep their
// own polls and reload when `epoch` changes.

export const BACKEND_NAMES: Record<ProxyBackendId, string> = { wireguard: 'WireGuard', clash: 'Clash' }

export const backendOf = (s: ProxyStatus, id: ProxyBackendId) => s.backends.find(b => b.id === id)

interface ProxyState {
  status: ProxyStatus | null
  error: string
  /** Where a running switch goes. */
  pending: ProxyChoice | null
  refresh: () => Promise<void>
  /** Asks, then makes `target` the only backend on; true when it switched. */
  switchTo: (target: ProxyChoice) => Promise<boolean>
  /** Counts finished switches. */
  epoch: number
}

const Ctx = createContext<ProxyState | null>(null)

export function useProxy() {
  const value = useContext(Ctx)
  if (!value) throw new Error('useProxy outside ProxyProvider') // i18n-ignore: developer error
  return value
}

/** Turning one on from off goes ahead; replacing one or turning off asks first. */
function ask(s: ProxyStatus, target: ProxyChoice) {
  // In a conflict, choosing WireGuard turns Clash off.
  if (target === 'off' || (s.conflict && target === 'wireguard')) {
    const name = target === 'wireguard' ? 'Clash' : s.conflict ? 'WireGuard + Clash' : BACKEND_NAMES[s.active as ProxyBackendId]
    return confirm({ title: t('Turn {name} off?', { name }), body: t('Settings are kept. Forwarded traffic pauses briefly while rules are removed; then all devices use the normal network.'), confirmLabel: t('Turn off') })
  }
  if (s.active === 'off') return Promise.resolve(true)
  return confirm(target === 'clash'
    ? { title: t('Switch from WireGuard to Clash?'), body: t('WireGuard will be turned off and its device choices kept. Proxy devices briefly lose internet while rules are replaced.'), confirmLabel: t('Switch to Clash') }
    : { title: t('Switch from Clash to WireGuard?'), body: t('Clash will be turned off and its settings kept. Tunnel devices briefly lose internet while rules are replaced.'), confirmLabel: t('Switch to WireGuard') })
}

export function ProxyProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<ProxyStatus | null>(null)
  const [error, setError] = useState('')
  const [pending, setPending] = useState<ProxyChoice | null>(null)
  const [epoch, setEpoch] = useState(0)
  const busy = useRef(false)
  const mounted = useRef(true)
  const generation = useRef(0)
  const latest = useRef<ProxyStatus | null>(null)

  const refresh = useCallback(async () => {
    if (busy.current) return
    const current = generation.current
    try {
      const s = await api.proxy()
      if (!mounted.current || busy.current || current !== generation.current) return
      latest.current = s; setStatus(s); setError('')
    } catch (e) {
      // Agents older than the selector answer 404 "not found".
      if (mounted.current && current === generation.current) setError(e instanceof ApiError && e.status === 404 ? t('This agent cannot switch the proxy; deploy the latest agent.') : e instanceof Error ? e.message : t('Unable to load proxy status'))
    }
  }, [])

  useEffect(() => {
    mounted.current = true
    void refresh()
    const timer = window.setInterval(() => { if (!document.hidden) void refresh() }, 5000)
    return () => { mounted.current = false; window.clearInterval(timer) }
  }, [refresh])

  const switchTo = useCallback(async (target: ProxyChoice) => {
    const s = latest.current
    if (!s || busy.current || (target === s.active && !s.conflict)) return false
    if (!(await ask(s, target)) || busy.current) return false
    const revision = (id: ProxyBackendId) => backendOf(s, id)?.revision ?? 0
    generation.current += 1
    busy.current = true; setPending(target)
    let done = false
    try {
      // With both saved as on, the agent turns off the one not chosen at its own revision.
      const next = await api.proxySet(target === 'off'
        ? { backend: target, expected_revision: revision(s.active as ProxyBackendId), active: s.active as ProxyBackendId }
        : { backend: target, expected_revision: revision(target) })
      if (mounted.current) { latest.current = next; setStatus(next); setError('') }
      done = true
      toast(target === 'off' ? t('Proxy off. All devices use the normal network.') : target === 'wireguard' ? t('WireGuard policy applied. Waiting for peer handshake.') : t('Clash is on.'))
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) toast(t('Proxy settings changed elsewhere. The latest state is shown; try again.'), 'err')
      else toastError(e, t('Unable to switch proxy'))
    } finally {
      busy.current = false
      if (mounted.current) { setPending(null); setEpoch(n => n + 1) }
    }
    if (!done) await refresh()
    return done
  }, [refresh])

  return <Ctx.Provider value={{ status, error, pending, refresh, switchTo, epoch }}>{children}</Ctx.Provider>
}
