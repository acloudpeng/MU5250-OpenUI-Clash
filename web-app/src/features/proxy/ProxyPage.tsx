import { useState } from 'react'
import { Tabs } from '../../ui/Tabs'
import { ClashProvider } from './ClashContext'
import OverviewTab from './OverviewTab'
import SubscriptionsTab from './SubscriptionsTab'
import ProxiesTab from './ProxiesTab'
import DevicesTab from './DevicesTab'
import ConnectionsTab from './ConnectionsTab'
import LogsTab from './LogsTab'

type Tab = 'overview' | 'subscriptions' | 'proxies' | 'devices' | 'connections' | 'logs'

export default function ProxyPage() {
  const [tab, setTab] = useState<Tab>('overview')

  return (
    <ClashProvider>
      <div className="space-y-4">
        <div>
          <h1 className="text-xl font-bold text-ink">Proxy</h1>
          <p className="mt-0.5 text-[13px] text-ink2">Clash.Meta (mihomo) subscriptions, nodes and device routing</p>
        </div>

        <Tabs
          tabs={[
            { id: 'overview', label: 'Overview' },
            { id: 'subscriptions', label: 'Subscriptions' },
            { id: 'proxies', label: 'Nodes' },
            { id: 'devices', label: 'Devices' },
            { id: 'connections', label: 'Connections' },
            { id: 'logs', label: 'Logs' },
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
