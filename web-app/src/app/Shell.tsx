/* eslint-disable react-refresh/only-export-components */
import { Suspense, useEffect, useState, type ReactNode } from 'react'
import { useAlerts } from './HomeContext'
import { IActivity, IGauge, IGlobe, IHome, ISim, ISignal, IX, IMoon, ISun, IShield, IMonitor } from '../icons'
import { Spinner } from '../ui/primitives'
import { N_, t, type Lang } from '../i18n'

export type Group = 'home' | 'signal' | 'network' | 'wireguard' | 'proxy' | 'modem' | 'devui' | 'system'

export const NAV: { id: Group; label: string; icon: (p: { size?: number; className?: string }) => ReactNode }[] = [
  { id: 'home', label: N_('Home'), icon: (p) => <IHome {...p} /> },
  { id: 'signal', label: N_('Signal'), icon: (p) => <ISignal {...p} /> },
  { id: 'network', label: N_('Network'), icon: (p) => <IGlobe {...p} /> },
  { id: 'wireguard', label: N_('WireGuard'), icon: (p) => <IShield {...p} /> },
  { id: 'proxy', label: N_('Proxy'), icon: (p) => <IActivity {...p} /> },
  { id: 'modem', label: N_('Modem'), icon: (p) => <ISim {...p} /> },
  { id: 'devui', label: N_('DevUI'), icon: (p) => <IMonitor {...p} /> },
  { id: 'system', label: N_('System'), icon: (p) => <IGauge {...p} /> },
]

const GROUP_TITLES: Record<Group, string> = {
  home: N_('Home'),
  signal: N_('Signal'),
  network: N_('Network'),
  wireguard: N_('WireGuard'),
  proxy: N_('Proxy'),
  modem: N_('Modem'),
  devui: N_('DevUI'),
  system: N_('System'),
}

// ── Alert banner (fed by the home poll — zero extra requests) ─────────────────

function AlertBanner() {
  const alerts = useAlerts()
  const [dismissed, setDismissed] = useState<Set<string>>(new Set())

  const visible = alerts.filter((a) => !dismissed.has(a.message))
  if (visible.length === 0) return null

  return (
    <div className="mb-4 space-y-1.5">
      {visible.map((a) => (
        <div
          key={a.message}
          className={`flex items-center gap-2.5 rounded-lg border px-3 py-2 text-[13px] font-medium ${
            a.level === 'error'
              ? 'border-danger/25 bg-danger/8 text-danger'
              : 'border-warn/25 bg-warn/8 text-warn'
          }`}
          role="alert"
        >
          <span className="min-w-0 flex-1">{a.message}</span>
          <button
            onClick={() => setDismissed((prev) => new Set(prev).add(a.message))}
            className="shrink-0 opacity-60 transition-opacity hover:opacity-100"
            aria-label={t('Dismiss')}
          >
            <IX size={14} />
          </button>
        </div>
      ))}
    </div>
  )
}

// ── Language switch ───────────────────────────────────────────────────────────

/** Shows the language it switches to, in that language. */
export function LangButton({ lang, onToggle }: { lang: Lang; onToggle: () => void }) {
  return (
    <button
      onClick={onToggle}
      className="flex h-8 items-center rounded-lg px-2 text-[12px] font-semibold text-ink2 transition-colors hover:bg-surface2 hover:text-ink"
      // i18n-ignore: named in the language it switches to
      aria-label={lang === 'zh' ? 'Switch to English' : '切换到中文'}
      lang={lang === 'zh' ? 'en' : 'zh-CN'}
    >
      {lang === 'zh' ? 'EN' : '中文'}
    </button>
  )
}

// ── Shell ─────────────────────────────────────────────────────────────────────

export default function Shell({
  group,
  onNavigate,
  theme,
  onToggleTheme,
  lang,
  onToggleLang,
  children,
}: {
  group: Group
  onNavigate: (g: Group) => void
  theme: 'light' | 'dark'
  onToggleTheme: () => void
  lang: Lang
  onToggleLang: () => void
  children: ReactNode
}) {
  // Lock body scroll while nothing needs it; keeps mobile address bar behavior sane.
  useEffect(() => {
    document.body.style.overflow = ''
  }, [])

  const themeIcon =
    theme === 'dark' ? <ISun size={17} /> : <IMoon size={17} />

  return (
    <div className="flex h-full bg-bg">
      {/* Desktop sidebar */}
      <aside className="hidden w-56 shrink-0 flex-col border-r border-line/8 bg-surface lg:flex">
        <div className="flex items-center gap-2.5 px-5 pb-5 pt-6">
          <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-accent text-white">
            <ISignal size={17} />
          </div>
          <div className="min-w-0">
            <p className="truncate text-[13px] font-bold text-ink">OpenUI + WG + Clash</p>
            <p className="tnum truncate text-[11px] text-ink3">{window.location.hostname}</p>
          </div>
        </div>

        <nav className="flex-1 space-y-0.5 px-3">
          {NAV.map((item) => (
            <button
              key={item.id}
              onClick={() => onNavigate(item.id)}
              className={`flex w-full items-center gap-2.5 rounded-lg px-3 py-2 text-[13px] font-semibold transition-colors ${
                group === item.id
                  ? 'bg-accent/10 text-accent'
                  : 'text-ink2 hover:bg-surface2 hover:text-ink'
              }`}
            >
              {item.icon({ size: 17 })}
              {t(item.label)}
            </button>
          ))}
        </nav>

        <div className="flex items-center justify-between border-t border-line/8 px-5 py-3">
          <button
            onClick={onToggleTheme}
            className="flex items-center gap-2 text-[12px] font-medium text-ink2 transition-colors hover:text-ink"
          >
            {themeIcon}
            {theme === 'dark' ? t('Light mode') : t('Dark mode')}
          </button>
          <LangButton lang={lang} onToggle={onToggleLang} />
        </div>
      </aside>

      {/* Main column */}
      <div className="flex min-w-0 flex-1 flex-col">
        {/* Mobile top bar */}
        <header
          className="flex min-h-12 shrink-0 items-center justify-between border-b border-line/8 bg-surface px-4 lg:hidden"
          style={{ paddingTop: 'env(safe-area-inset-top)' }}
        >
          <span className="text-sm font-bold text-ink">{t(GROUP_TITLES[group])}</span>
          <div className="flex items-center gap-1">
          <LangButton lang={lang} onToggle={onToggleLang} />
          <button
            onClick={onToggleTheme}
            className="flex h-8 w-8 items-center justify-center rounded-lg text-ink2 transition-colors hover:bg-surface2 hover:text-ink"
            aria-label={t('Toggle theme')}
          >
            {themeIcon}
          </button>
          </div>
        </header>

        <main className="min-h-0 flex-1 overflow-y-auto">
          <div className="mx-auto w-full max-w-shell px-4 pb-24 pt-4 lg:px-6 lg:pb-10 lg:pt-6">
            {import.meta.env.MODE === 'demo' && <p className="mb-4 rounded-lg bg-amber-100 px-3 py-2 text-xs font-semibold text-amber-800">{t('Demo data · OpenUI with WireGuard & Clash · No device connected')}</p>}
            <AlertBanner />
            <Suspense
              fallback={
                <div className="flex justify-center py-20 text-ink3">
                  <Spinner size={22} />
                </div>
              }
            >
              {children}
            </Suspense>
          </div>
        </main>

        {/* Mobile bottom tabs */}
        <nav
          className="fixed inset-x-0 bottom-0 z-30 border-t border-line/8 bg-surface lg:hidden"
          style={{ paddingBottom: 'env(safe-area-inset-bottom)' }}
        >
          <div className="mx-auto flex max-w-md items-stretch justify-around">
            {NAV.map((item) => {
              const active = group === item.id
              return (
                <button
                  key={item.id}
                  onClick={() => onNavigate(item.id)}
                  className={`flex flex-1 flex-col items-center gap-0.5 pb-1.5 pt-2 text-[10px] font-semibold transition-colors ${
                    active ? 'text-accent' : 'text-ink3 hover:text-ink2'
                  }`}
                >
                  {item.icon({ size: 20 })}
                  {t(item.label)}
                </button>
              )
            })}
          </div>
        </nav>
      </div>
    </div>
  )
}
