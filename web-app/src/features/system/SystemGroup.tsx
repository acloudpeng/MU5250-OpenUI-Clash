import { useState } from 'react'
import { t } from '../../i18n'
import { Tabs } from '../../ui/Tabs'
import MetricsTab from './MetricsTab'
import ToolsTab from './ToolsTab'
import { PageHeader } from '../../ui/primitives'
import SettingsTab from './SettingsTab'

type Tab = 'metrics' | 'tools' | 'settings'

export default function SystemGroup({ onLogout }: { onLogout: () => void }) {
  const [tab, setTab] = useState<Tab>('metrics')

  return (
    <div className="space-y-4">
      <PageHeader title={t('System')} sub={t('Health metrics, diagnostic tools and device controls')} />

      <Tabs
        tabs={[
          { id: 'metrics', label: t('Metrics') },
          { id: 'tools', label: t('Tools') },
          { id: 'settings', label: t('Settings') },
        ]}
        active={tab}
        onChange={setTab}
      />

      {tab === 'metrics' && <MetricsTab />}
      {tab === 'tools' && <ToolsTab />}
      {tab === 'settings' && <SettingsTab onLogout={onLogout} />}
    </div>
  )
}
