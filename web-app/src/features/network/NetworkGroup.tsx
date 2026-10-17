import { useState } from 'react'
import { Tabs } from '../../ui/Tabs'
import ClientsTab from './ClientsTab'
import WifiTab from './WifiTab'
import UplinkTab from './UplinkTab'
import RouterTab from './RouterTab'
import { PageHeader } from '../../ui/primitives'
import { t } from '../../i18n'

type Tab = 'clients' | 'wifi' | 'uplink' | 'router'

export default function NetworkGroup() {
  const [tab, setTab] = useState<Tab>('clients')

  return (
    <div className="space-y-4">
      <PageHeader title={t('Network')} sub={t('Connected clients, Wi-Fi, Wi-Fi relay and router settings')} />

      <Tabs
        tabs={[
          { id: 'clients', label: t('Clients') },
          { id: 'wifi', label: 'Wi-Fi' },
          { id: 'uplink', label: t('Wi-Fi relay') },
          { id: 'router', label: t('Router') },
        ]}
        active={tab}
        onChange={setTab}
      />

      {tab === 'clients' && <ClientsTab />}
      {tab === 'wifi' && <WifiTab />}
      {tab === 'uplink' && <UplinkTab />}
      {tab === 'router' && <RouterTab />}
    </div>
  )
}
