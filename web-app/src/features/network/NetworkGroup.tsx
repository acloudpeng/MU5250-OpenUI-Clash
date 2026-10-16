import { useState } from 'react'
import { Tabs } from '../../ui/Tabs'
import ClientsTab from './ClientsTab'
import WifiTab from './WifiTab'
import UplinkTab from './UplinkTab'
import RouterTab from './RouterTab'
import { t } from '../../i18n'

type Tab = 'clients' | 'wifi' | 'uplink' | 'router'

export default function NetworkGroup() {
  const [tab, setTab] = useState<Tab>('clients')

  return (
    <div className="space-y-4">
      <div>
        <h1 className="text-xl font-bold text-ink">{t('Network')}</h1>
        <p className="mt-0.5 text-[13px] text-ink2">{t('Connected clients, Wi-Fi, Wi-Fi relay and router settings')}</p>
      </div>

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
