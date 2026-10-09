import type { ProxyBackendId, ProxyChoice } from '../../types'
import { Segmented } from '../../ui/controls'
import { Card, Chip, type ChipTone } from '../../ui/primitives'
import { BACKEND_NAMES, backendOf, useProxy } from './ProxyContext'
import { t, tm } from '../../i18n'

const IDS: ProxyBackendId[] = ['wireguard', 'clash']

/** Off / WireGuard / Clash: which backend routes the chosen devices. */
export default function BackendSelector({ onSwitched }: { onSwitched: (target: ProxyChoice) => void }) {
  const { status, error, pending, switchTo } = useProxy()
  const current = status && status.active !== 'off' ? backendOf(status, status.active) : undefined
  const [tone, label]: [ChipTone, string] = pending ? ['accent', t('Switching…')]
    : !current ? ['default', t('Off')]
    : current.error ? ['danger', t('Needs attention')]
    : current.id === 'wireguard' ? ['accent', t('On')]
    : current.running ? ['ok', t('Running')] : ['warn', t('Starting')]
  const line = pending ? t('Applying. This can take up to two minutes.')
    : !current ? t('All devices use the normal network.')
    : current.id === 'wireguard' ? t('Devices chosen in the WireGuard settings use the tunnel.')
    : t('Devices chosen in the Clash settings go through the core.')
  // Why an option is unavailable; the active one stays selectable to show its state.
  const blocked = status ? IDS.flatMap(id => {
    const b = backendOf(status, id)
    return b && !b.ready && b.reason && id !== status.active ? [{ id, reason: b.reason }] : []
  }) : []

  return (
    <Card title={t('Active proxy')} action={status ? <Chip tone={tone}>{label}</Chip> : undefined}>
      <div className="space-y-3">
        {status ? <>
          <Segmented<ProxyChoice> value={pending ?? status.active} disabled={!!pending}
            options={[{ value: 'off', label: t('Off') }, ...IDS.map(id => ({ value: id, label: BACKEND_NAMES[id], disabled: !backendOf(status, id)?.ready && id !== status.active }))]}
            onChange={v => void switchTo(v).then(ok => { if (ok) onSwitched(v) })} />
          <p className="text-xs text-ink2">{line}</p>
        </> : !error && <p className="text-sm text-ink2">{t('Loading…')}</p>}
        {error && <p role="alert" className="rounded-lg bg-danger/10 p-3 text-[13px] text-danger">{tm(error)}</p>}
        {current?.error && !pending && <p role="alert" className="rounded-lg bg-danger/10 p-3 text-[13px] text-danger">{tm(current.error)}</p>}
        {status?.conflict && <p role="alert" className="rounded-lg bg-warn/10 p-3 text-[13px] text-warn">{t('WireGuard and Clash are both saved as on. WireGuard is in effect and Clash stays off; choose one to turn the other off.')}</p>}
        {blocked.map(b => <p key={b.id} className="text-xs text-ink2">{t('{name}: {reason}', { name: BACKEND_NAMES[b.id], reason: tm(b.reason) })}</p>)}
        <p className="text-xs text-ink3">{t('Only one can be on. Switching turns the other off and keeps its settings; devices briefly lose internet while rules are replaced.')}</p>
      </div>
    </Card>
  )
}
