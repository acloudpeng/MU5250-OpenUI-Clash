import { useState } from 'react'
import { t } from '../../i18n'
import { Tabs } from '../../ui/Tabs'
import Overview from './Overview'
import { PageHeader } from '../../ui/primitives'
import Locking from './Locking'

type Tab = 'overview' | 'locking'

export default function SignalGroup() {
  const [tab, setTab] = useState<Tab>('overview')

  return (
    <div className="space-y-4">
      <PageHeader title={t('Signal')} sub={t('Live radio metrics, band and cell locking')} />

      <Tabs
        tabs={[
          { id: 'overview', label: t('Overview') },
          { id: 'locking', label: t('Mode & Locking') },
        ]}
        active={tab}
        onChange={setTab}
      />

      {tab === 'overview' ? <Overview /> : <Locking />}
    </div>
  )
}
