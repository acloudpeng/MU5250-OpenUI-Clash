/* eslint-disable react-refresh/only-export-components */
import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from 'react'
import { api } from '../../data/api'
import type { ClashStatus, ClashUpdate } from '../../types'
import { toastError } from '../../ui/feedback'
import { useProxy } from './ProxyContext'
import { t } from '../../i18n'

interface ClashState {
  status: ClashStatus | null
  error: string
  busy: boolean
  refresh: () => Promise<void>
  /** Sends one change against the latest revision (or `revision`, for edits made against an older
   * view). Turning Clash on or off is the backend selector's job. */
  update: (change: Omit<ClashUpdate, 'expected_revision' | 'enabled'>, failure?: string, revision?: number) => Promise<ClashStatus | null>
  /** Runs another request that returns fresh status (e.g. subscription refresh). */
  run: (task: () => Promise<ClashStatus>, failure: string) => Promise<ClashStatus | null>
}

const Ctx = createContext<ClashState | null>(null)

export function useClash() {
  const value = useContext(Ctx)
  if (!value) throw new Error('useClash outside ClashProvider') // i18n-ignore: developer error
  return value
}

export function ClashProvider({ children }: { children: ReactNode }) {
  const { epoch, refresh: refreshProxy } = useProxy()
  const [status, setStatus] = useState<ClashStatus | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const busyRef = useRef(false)
  const mounted = useRef(true)
  const generation = useRef(0)
  const latest = useRef<ClashStatus | null>(null)

  const refresh = useCallback(async () => {
    if (busyRef.current) return
    const current = generation.current
    try {
      const s = await api.clash()
      if (!mounted.current || busyRef.current || current !== generation.current) return
      latest.current = s; setStatus(s); setError('')
    } catch (e) {
      if (mounted.current && current === generation.current) setError(e instanceof Error ? e.message : t('Unable to load Clash'))
    }
  }, [])

  useEffect(() => {
    mounted.current = true
    void refresh()
    const timer = window.setInterval(() => { if (!document.hidden) void refresh() }, 5000)
    return () => { mounted.current = false; window.clearInterval(timer) }
  }, [refresh])

  // Reload after a switch of the active backend.
  const seen = useRef(epoch)
  useEffect(() => {
    if (seen.current === epoch) return
    seen.current = epoch
    generation.current += 1
    void refresh()
  }, [epoch, refresh])

  const run = useCallback(async (task: () => Promise<ClashStatus>, failure: string) => {
    if (busyRef.current) return null
    generation.current += 1
    busyRef.current = true; setBusy(true); setError('')
    try {
      const s = await task()
      if (mounted.current) { latest.current = s; setStatus(s) }
      // Readiness and the revision the selector switches against may have changed.
      void refreshProxy()
      return s
    } catch (e) {
      if (mounted.current) { setError(e instanceof Error ? e.message : failure); toastError(e, failure) }
      return null
    } finally {
      busyRef.current = false
      if (mounted.current) setBusy(false)
    }
  }, [refreshProxy])

  const update = useCallback(async (change: Omit<ClashUpdate, 'expected_revision' | 'enabled'>, failure = t('Clash update failed'), revision?: number) => {
    const s = latest.current
    if (!s) return null
    return run(() => api.clashSet({ ...change, expected_revision: revision ?? s.revision }), failure)
  }, [run])

  return <Ctx.Provider value={{ status, error, busy, refresh, update, run }}>{children}</Ctx.Provider>
}
