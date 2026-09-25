import { useState } from 'react'
import { Tabs } from '../../ui/Tabs'
import { ClashProvider } from './ClashContext'
import OverviewTab from './OverviewTab'
import SubscriptionsTab from './SubscriptionsTab'
import ProxiesTab from './ProxiesTab'
import DevicesTab from './DevicesTab'
import ConnectionsTab from './ConnectionsTab'
import LogsTab from './LogsTab'
import { t } from '../../i18n'

type Tab = 'overview' | 'subscriptions' | 'proxies' | 'devices' | 'connections' | 'logs'

export default function ProxyPage() {
  const [tab, setTab] = useState<Tab>('overview')

  return (
    <ClashProvider>
      <div className="space-y-4">
        <div>
          <h1 className="text-xl font-bold text-ink">{t('Proxy')}</h1>
          <p className="mt-0.5 text-[13px] text-ink2">{t('Clash.Meta (mihomo) subscriptions, nodes and device routing')}</p>
        </div>

        <Tabs
          tabs={[
            { id: 'overview', label: t('Overview') },
            { id: 'subscriptions', label: t('Subscriptions') },
            { id: 'proxies', label: t('Nodes') },
            { id: 'devices', label: t('Devices') },
            { id: 'connections', label: t('Connections') },
            { id: 'logs', label: t('Logs') },
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
      </div>
    </ClashProvider>
  )
}
