import { useState } from 'react'
import type { ProxyBackendId } from '../../types'
import { Segmented } from '../../ui/controls'
import { Tabs } from '../../ui/Tabs'
import { ProxyProvider, useProxy } from './ProxyContext'
import BackendSelector from './BackendSelector'
import { ClashProvider } from './ClashContext'
import OverviewTab from './OverviewTab'
import SubscriptionsTab from './SubscriptionsTab'
import ProxiesTab from './ProxiesTab'
import DevicesTab from './DevicesTab'
import ConnectionsTab from './ConnectionsTab'
import LogsTab from './LogsTab'
import AdvancedTab from './AdvancedTab'
import WireGuardTab from './wireguard/WireGuardTab'
import { t } from '../../i18n'

type ClashTab = 'overview' | 'subscriptions' | 'proxies' | 'devices' | 'connections' | 'logs' | 'advanced'

function ClashView() {
  const [tab, setTab] = useState<ClashTab>('overview')

  return (
    <ClashProvider>
      <Tabs
        tabs={[
          { id: 'overview', label: t('Overview') },
          { id: 'subscriptions', label: t('Subscriptions') },
          { id: 'proxies', label: t('Nodes') },
          { id: 'devices', label: t('Devices') },
          { id: 'connections', label: t('Connections') },
          { id: 'logs', label: t('Logs') },
          { id: 'advanced', label: t('Advanced') },
        ]}
        active={tab}
        onChange={setTab}
      />

      {tab === 'overview' && <OverviewTab />}
      {tab === 'subscriptions' && <SubscriptionsTab />}
      {tab === 'proxies' && <ProxiesTab />}
      {tab === 'devices' && <DevicesTab />}
      {tab === 'connections' && <ConnectionsTab />}
      {tab === 'logs' && <LogsTab />}
      {tab === 'advanced' && <AdvancedTab />}
    </ClashProvider>
  )
}

function Content() {
  const { status, error } = useProxy()
  // Which settings are shown: the user's pick, else the active backend's (Clash when off).
  const [view, setView] = useState<ProxyBackendId | null>(null)
  const shown = view ?? (status ? (status.active === 'wireguard' ? 'wireguard' : 'clash') : error ? 'clash' : null)

  return (
    <div className="space-y-4">
      <div>
        <h1 className="text-xl font-bold text-ink">{t('Proxy')}</h1>
        <p className="mt-0.5 text-[13px] text-ink2">{t('Choose WireGuard or Clash for your devices and configure each')}</p>
      </div>

      {/* After a switch, show the backend just turned on. */}
      <BackendSelector onSwitched={target => setView(target === 'off' ? shown : target)} />

      {shown && <>
        <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
          <h2 className="text-[13px] font-semibold text-ink">{t('Configure')}</h2>
          <Segmented<ProxyBackendId> value={shown} onChange={setView}
            options={[{ value: 'wireguard', label: 'WireGuard' }, { value: 'clash', label: 'Clash' }]} />
        </div>
        {shown === 'clash' ? <ClashView /> : <WireGuardTab />}
      </>}
    </div>
  )
}

/** Both proxy backends: the active-backend switch, then each one's settings. */
export default function ProxyPage() {
  return (
    <ProxyProvider>
      <Content />
    </ProxyProvider>
  )
}
