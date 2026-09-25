import { useEffect, useRef, useState } from 'react'
import { api } from '../../data/api'
import { usePoll } from '../../data/poll'
import { Button } from '../../ui/controls'
import { Card } from '../../ui/primitives'
import { useClash } from './ClashContext'
import { N_, t, tm } from '../../i18n'

const LEVEL = /level=(\w+)/
const TONE: Record<string, string> = { error: 'text-danger', fatal: 'text-danger', warning: 'text-warn', debug: 'text-ink3' }
const LEVEL_LABELS: Record<string, string> = { silent: N_('Silent'), error: N_('Error'), warning: N_('Warning'), info: N_('Info'), debug: N_('Debug') }

export default function LogsTab() {
  const { status } = useClash()
  const [follow, setFollow] = useState(true)
  const { data, error, refresh } = usePoll('clash-logs', api.clashLogs, 3000, follow)
  const lines = data?.lines ?? []
  const box = useRef<HTMLPreElement>(null)
  useEffect(() => {
    if (follow && box.current) box.current.scrollTop = box.current.scrollHeight
  }, [data, follow])

  return (
    <Card title={t('Core log')} action={<div className="flex gap-2">
      <Button size="sm" variant="ghost" onClick={() => setFollow(f => !f)}>{follow ? t('Pause') : t('Follow')}</Button>
      <Button size="sm" variant="ghost" disabled={!follow} onClick={refresh}>{t('Reload')}</Button>
    </div>}>
      <div className="space-y-3">
        <p className="text-xs text-ink2">{t('The last 300 lines from the core, kept in memory only.')} {t('Current level:')} <strong className="text-ink">{status?.log_level ? t(LEVEL_LABELS[status.log_level] ?? status.log_level) : '—'}</strong> {t('(change it under Overview → Advanced).')}</p>
        {error && <p role="alert" className="rounded-lg bg-danger/10 p-3 text-[13px] text-danger">{tm(error)}</p>}
        <pre ref={box} className="max-h-[60vh] overflow-auto whitespace-pre-wrap break-all rounded-lg bg-surface2/60 p-3 font-mono text-[11px] leading-relaxed text-ink2">
          {lines.length === 0 ? t('No log lines yet.') : lines.map((l, i) => <div key={i} className={TONE[LEVEL.exec(l)?.[1] ?? ''] ?? ''}>{l}</div>)}
        </pre>
      </div>
    </Card>
  )
}
