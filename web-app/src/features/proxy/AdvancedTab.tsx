import type { ClashDnsMode, ClashLogLevel, ClashStack } from '../../types'
import { Field, Select } from '../../ui/controls'
import { Card } from '../../ui/primitives'
import { useClash } from './ClashContext'
import CoreCard from './CoreCard'
import { t } from '../../i18n'

export default function AdvancedTab() {
  const { status, busy, update } = useClash()
  if (!status) return <p className="text-sm text-ink2">{t('Loading…')}</p>

  return (
    <div className="space-y-4">
      <Card title={t('Advanced')}>
        <div className="grid gap-4 sm:grid-cols-3">
          <Field label={t('TUN stack')} hint={t('System is fastest; gVisor is more compatible. Applying briefly pauses proxy traffic.')}>
            <Select aria-label={t('TUN stack')} value={status.stack} disabled={busy} onChange={e => void update({ stack: e.target.value as ClashStack }, t('Unable to change TUN stack'))}>
              <option value="system">{t('System')}</option>
              <option value="gvisor">{t('gVisor')}</option>
              <option value="mixed">{t('Mixed (TCP system, UDP gVisor)')}</option>
            </Select>
          </Field>
          <Field label={t('DNS mode')} hint={t('Fake-IP avoids DNS leaks and is recommended. Applying briefly pauses proxy traffic.')}>
            <Select aria-label={t('DNS mode')} value={status.dns_mode} disabled={busy} onChange={e => void update({ dns_mode: e.target.value as ClashDnsMode }, t('Unable to change DNS mode'))}>
              <option value="fake-ip">{t('Fake-IP')}</option>
              <option value="redir-host">{t('Redir-host')}</option>
            </Select>
          </Field>
          <Field label={t('Log level')} hint={t('Info or debug shows each connection in Logs.')}>
            <Select aria-label={t('Log level')} value={status.log_level} disabled={busy} onChange={e => void update({ log_level: e.target.value as ClashLogLevel }, t('Unable to change log level'))}>
              <option value="silent">{t('Silent')}</option>
              <option value="error">{t('Error')}</option>
              <option value="warning">{t('Warning')}</option>
              <option value="info">{t('Info')}</option>
              <option value="debug">{t('Debug')}</option>
            </Select>
          </Field>
        </div>
        <p className="mt-3 text-xs text-ink3">{t('IPv6 internet and IPv6 DNS are blocked for proxy devices so nothing bypasses the core. Router management always stays reachable.')}</p>
      </Card>
      <CoreCard />
    </div>
  )
}
