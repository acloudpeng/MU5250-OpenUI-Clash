import { useState } from 'react'
import { t } from '../../i18n'
import { Tabs } from '../../ui/Tabs'
import ApnTab from './ApnTab'
import DataTab from './DataTab'
import TtlTab from './TtlTab'
import SmsTab from './SmsTab'
import { PageHeader } from '../../ui/primitives'
import EsimTab from './EsimTab'

type Tab = 'apn' | 'esim' | 'data' | 'ttl' | 'sms'

export default function ModemGroup() {
  const [tab, setTab] = useState<Tab>('apn')

  return (
    <div className="space-y-4">
      <PageHeader title={t('Modem')} sub={t('APN profiles, eSIM, data usage, TTL and SMS')} />

      <Tabs
        tabs={[
          { id: 'apn', label: 'APN' },
          { id: 'esim', label: 'eSIM' },
          { id: 'data', label: t('Data') },
          { id: 'ttl', label: 'TTL' },
          { id: 'sms', label: t('SMS') },
        ]}
        active={tab}
        onChange={setTab}
      />

      {tab === 'apn' && <ApnTab />}
      {tab === 'esim' && <EsimTab />}
      {tab === 'data' && <DataTab />}
      {tab === 'ttl' && <TtlTab />}
      {tab === 'sms' && <SmsTab />}
    </div>
  )
}
