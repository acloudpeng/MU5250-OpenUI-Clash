/* eslint-disable react-refresh/only-export-components */
import { Suspense, useEffect, useRef, useState, type ReactNode } from 'react'
import { useAlerts, useHome } from './HomeContext'
import {
  IActivity, IBattery, IBolt, ICpu, IGauge, IGlobe, IHome, IMonitor, IMoon, IPower, IRestart, ISignal, ISim, ISun, IThermo, IX,
} from '../icons'
import { Spinner } from '../ui/primitives'
import { powerAction } from './power'
import { formatSpeed } from '../format'
import { N_, t, type Lang } from '../i18n'

export type Group = 'home' | 'signal' | 'network' | 'proxy' | 'modem' | 'devui' | 'system'

type NavIcon = (p: { size?: number; className?: string }) => ReactNode

export const NAV: { id: Group; label: string; hint: string; icon: NavIcon }[] = [
  { id: 'home', label: N_('Home'), hint: N_('Status at a glance'), icon: (p) => <IHome {...p} /> },
  { id: 'signal', label: N_('Signal'), hint: N_('Radio metrics and locking'), icon: (p) => <ISignal {...p} /> },
  { id: 'network', label: N_('Network'), hint: N_('Clients, Wi-Fi and relay'), icon: (p) => <IGlobe {...p} /> },
  { id: 'proxy', label: N_('Proxy'), hint: N_('WireGuard and Clash'), icon: (p) => <IActivity {...p} /> },
  { id: 'modem', label: N_('Modem'), hint: N_('APN, eSIM, data, TTL and SMS'), icon: (p) => <ISim {...p} /> },
  { id: 'devui', label: N_('DevUI'), hint: N_('The device screen'), icon: (p) => <IMonitor {...p} /> },
  { id: 'system', label: N_('System'), hint: N_('Metrics, tools and settings'), icon: (p) => <IGauge {...p} /> },
]

/** The phone's bottom bar: these four, then "More" for the rest. */
const PRIMARY: Group[] = ['home', 'signal', 'network', 'proxy']

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
          className={`flex items-center gap-2.5 rounded-xl border px-3 py-2 text-[13px] font-medium ${
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

// ── Status strip (fed by HomeContext — zero extra requests) ───────────────────

function StatusChip({ onClick, children, title }: { onClick?: () => void; children: ReactNode; title?: string }) {
  const cls = 'flex shrink-0 items-center gap-1.5 rounded-lg px-2.5 py-1 text-[12px] font-medium text-ink2'
  return onClick ? (
    <button onClick={onClick} title={title} className={`${cls} transition-colors hover:bg-surface2 hover:text-ink`}>
      {children}
    </button>
  ) : (
    <span title={title} className={cls}>{children}</span>
  )
}

/** Signal, speed, battery and load: always on screen, one poll for all of them. */
function StatusStrip({ onNavigate, className = '' }: { onNavigate: (g: Group) => void; className?: string }) {
  const { data } = useHome()
  if (!data) return null
  const sig = data.signal
  const batt = data.battery
  const spd = data.speed
  const cpu = data.cpu
  const mem = data.memory
  const temp = data.thermal?.cpu_temp_c
  const rsrp = sig?.lte_carriers?.[0]?.rsrp ?? sig?.nr_carriers?.[0]?.rsrp ?? sig?.rsrp
  const tone = rsrp == null ? 'text-ink3' : rsrp >= -95 ? 'text-ok' : rsrp >= -110 ? 'text-warn' : 'text-danger'

  return (
    <div className={`no-scrollbar flex min-w-0 items-center gap-0.5 overflow-x-auto ${className}`}>
      <StatusChip onClick={() => onNavigate('signal')} title={t('Signal')}>
        <ISignal size={14} className={tone} />
        <span className="font-semibold text-ink">{sig?.type ?? '—'}</span>
        {rsrp != null && <span className="tnum">{rsrp} dBm</span>}
      </StatusChip>
      {spd && (
        <StatusChip title={t('Throughput')}>
          <span className="tnum text-ok">↓ {formatSpeed(spd.rx_bps)}</span>
          <span className="tnum text-accent">↑ {formatSpeed(spd.tx_bps)}</span>
        </StatusChip>
      )}
      {batt && (
        <StatusChip onClick={() => onNavigate('system')} title={t('Battery')}>
          {batt.charging ? <IBolt size={14} className="text-ok" /> : <IBattery size={14} />}
          <span className="tnum font-semibold text-ink">{batt.percent}%</span>
        </StatusChip>
      )}
      <StatusChip onClick={() => onNavigate('system')} title={t('System')}>
        <ICpu size={14} />
        <span className="tnum">CPU {cpu ? `${cpu.overall.toFixed(0)}%` : '—'}</span>
        {mem && <span className="tnum">· RAM {Math.round(mem.usage_pct)}%</span>}
      </StatusChip>
      {temp != null && (
        <StatusChip onClick={() => onNavigate('system')} title={t('Temperatures')}>
          <IThermo size={14} className={temp >= 75 ? 'text-danger' : ''} />
          <span className="tnum">{temp.toFixed(0)}°C</span>
        </StatusChip>
      )}
    </div>
  )
}

// ── Power menu ────────────────────────────────────────────────────────────────

function PowerMenu() {
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false)
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    window.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      window.removeEventListener('keydown', onKey)
    }
  }, [open])

  const item = (action: 'reboot' | 'shutdown', icon: ReactNode, label: string) => (
    <button
      role="menuitem"
      onClick={() => {
        setOpen(false)
        void powerAction(action)
      }}
      className="flex w-full items-center gap-2.5 rounded-lg px-3 py-2 text-left text-[13px] font-medium text-ink transition-colors hover:bg-surface2"
    >
      <span className="text-danger">{icon}</span>
      {label}
    </button>
  )

  return (
    <div ref={ref} className="relative">
      <button
        onClick={() => setOpen((v) => !v)}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={t('Power menu')}
        title={t('Power menu')}
        className={`flex h-9 w-9 items-center justify-center rounded-xl transition-colors ${
          open ? 'bg-danger/10 text-danger' : 'text-ink2 hover:bg-surface2 hover:text-danger'
        }`}
      >
        <IPower size={18} />
      </button>
      {open && (
        <div role="menu" className="absolute right-0 top-11 z-40 w-48 rounded-xl border border-line/10 bg-surface p-1.5 shadow-pop">
          {item('reboot', <IRestart size={16} />, t('Reboot device'))}
          {item('shutdown', <IPower size={16} />, t('Shut down'))}
        </div>
      )}
    </div>
  )
}

// ── Language and theme ────────────────────────────────────────────────────────

/** Shows the language it switches to, in that language. */
export function LangButton({ lang, onToggle }: { lang: Lang; onToggle: () => void }) {
  return (
    <button
      onClick={onToggle}
      className="flex h-9 items-center rounded-xl px-2.5 text-[12px] font-semibold text-ink2 transition-colors hover:bg-surface2 hover:text-ink"
      // i18n-ignore: named in the language it switches to
      aria-label={lang === 'zh' ? 'Switch to English' : '切换到中文'}
      lang={lang === 'zh' ? 'en' : 'zh-CN'}
    >
      {lang === 'zh' ? 'EN' : '中文'}
    </button>
  )
}

function ThemeButton({ theme, onToggle }: { theme: 'light' | 'dark'; onToggle: () => void }) {
  return (
    <button
      onClick={onToggle}
      className="flex h-9 w-9 items-center justify-center rounded-xl text-ink2 transition-colors hover:bg-surface2 hover:text-ink"
      aria-label={t('Toggle theme')}
      title={theme === 'dark' ? t('Light mode') : t('Dark mode')}
    >
      {theme === 'dark' ? <ISun size={17} /> : <IMoon size={17} />}
    </button>
  )
}

// ── "More" sheet on phones ────────────────────────────────────────────────────

function MoreSheet({
  group,
  onNavigate,
  onClose,
}: {
  group: Group
  onNavigate: (g: Group) => void
  onClose: () => void
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])

  return (
    <div className="fixed inset-0 z-40 bg-black/40 lg:hidden" onClick={onClose}>
      <div
        className="absolute inset-x-0 bottom-0 rounded-t-2xl border-t border-line/10 bg-surface p-3 shadow-pop"
        style={{ paddingBottom: 'calc(env(safe-area-inset-bottom) + 12px)' }}
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-label={t('More')}
      >
        <div className="mx-auto mb-2 h-1 w-10 rounded-full bg-line/15" />
        <nav className="space-y-1">
          {NAV.filter((n) => !PRIMARY.includes(n.id)).map((item) => (
            <button
              key={item.id}
              onClick={() => {
                onNavigate(item.id)
                onClose()
              }}
              className={`flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left transition-colors ${
                group === item.id ? 'bg-accent/10 text-accent' : 'text-ink hover:bg-surface2'
              }`}
            >
              <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-xl bg-surface2">{item.icon({ size: 18 })}</span>
              <span className="min-w-0">
                <span className="block text-[14px] font-semibold">{t(item.label)}</span>
                <span className="block truncate text-[12px] text-ink3">{t(item.hint)}</span>
              </span>
            </button>
          ))}
        </nav>
      </div>
    </div>
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
  const [more, setMore] = useState(false)
  const mainRef = useRef<HTMLElement>(null)

  // A new group starts at the top.
  useEffect(() => {
    mainRef.current?.scrollTo({ top: 0 })
  }, [group])

  return (
    <div className="flex h-full bg-bg">
      {/* Desktop sidebar */}
      <aside className="hidden w-60 shrink-0 flex-col border-r border-line/8 bg-surface lg:flex">
        <div className="flex items-center gap-3 px-5 pb-6 pt-6">
          <div className="flex h-9 w-9 items-center justify-center rounded-xl bg-accent text-white shadow-card">
            <ISignal size={18} />
          </div>
          <div className="min-w-0">
            <p className="truncate text-[14px] font-bold text-ink">{t('Advanced admin')}</p>
            <p className="tnum truncate text-[11px] text-ink3">{window.location.hostname}</p>
          </div>
        </div>

        <nav className="flex-1 space-y-1 px-3">
          {NAV.map((item) => {
            const active = group === item.id
            return (
              <button
                key={item.id}
                onClick={() => onNavigate(item.id)}
                className={`flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-[13.5px] font-semibold transition-colors ${
                  active ? 'bg-accent/10 text-accent' : 'text-ink2 hover:bg-surface2 hover:text-ink'
                }`}
                aria-current={active ? 'page' : undefined}
              >
                {item.icon({ size: 18 })}
                {t(item.label)}
              </button>
            )
          })}
        </nav>

        <div className="flex items-center justify-between border-t border-line/8 px-3 py-3">
          <button
            onClick={onToggleTheme}
            className="flex h-9 items-center gap-2 rounded-xl px-2.5 text-[12px] font-medium text-ink2 transition-colors hover:bg-surface2 hover:text-ink"
          >
            {theme === 'dark' ? <ISun size={16} /> : <IMoon size={16} />}
            {theme === 'dark' ? t('Light mode') : t('Dark mode')}
          </button>
          <LangButton lang={lang} onToggle={onToggleLang} />
        </div>
      </aside>

      {/* Main column */}
      <div className="flex min-w-0 flex-1 flex-col">
        <main ref={mainRef} className="min-h-0 flex-1 overflow-y-auto">
          {/* Top bar: status everywhere, the power menu, and on phones language and theme. */}
          <header
            className="sticky top-0 z-30 border-b border-line/8 bg-bg/85 backdrop-blur-md"
            style={{ paddingTop: 'env(safe-area-inset-top)' }}
          >
            <div className="mx-auto flex h-14 w-full max-w-shell items-center gap-2 px-4 lg:px-6">
              <div className="flex shrink-0 items-center gap-2 lg:hidden">
                <div className="flex h-7 w-7 items-center justify-center rounded-lg bg-accent text-white">
                  <ISignal size={15} />
                </div>
                <span className="text-[14px] font-bold text-ink">{t('Advanced admin')}</span>
              </div>
              <StatusStrip onNavigate={onNavigate} className="-ml-2.5 hidden md:flex" />
              <div className="ml-auto flex shrink-0 items-center gap-0.5">
                <div className="flex items-center gap-0.5 lg:hidden">
                  <LangButton lang={lang} onToggle={onToggleLang} />
                  <ThemeButton theme={theme} onToggle={onToggleTheme} />
                </div>
                <PowerMenu />
              </div>
            </div>
            <div className="mx-auto max-w-shell px-2 pb-1.5 md:hidden">
              <StatusStrip onNavigate={onNavigate} />
            </div>
          </header>

          <div className="mx-auto w-full max-w-shell px-4 pb-28 pt-5 lg:px-6 lg:pb-10 lg:pt-6">
            {import.meta.env.MODE === 'demo' && <p className="mb-4 rounded-xl bg-amber-100 px-3 py-2 text-xs font-semibold text-amber-800">{t('Demo data · No device connected')}</p>}
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

        {/* Mobile bottom bar: four groups and "More" */}
        <nav
          className="fixed inset-x-0 bottom-0 z-30 border-t border-line/8 bg-surface/95 backdrop-blur-md lg:hidden"
          style={{ paddingBottom: 'env(safe-area-inset-bottom)' }}
        >
          <div className="mx-auto flex max-w-md items-stretch justify-around px-1">
            {NAV.filter((n) => PRIMARY.includes(n.id)).map((item) => {
              const active = group === item.id
              return (
                <button
                  key={item.id}
                  onClick={() => onNavigate(item.id)}
                  className={`flex flex-1 flex-col items-center gap-0.5 pb-1.5 pt-2 text-[10.5px] font-semibold transition-colors ${
                    active ? 'text-accent' : 'text-ink3 hover:text-ink2'
                  }`}
                  aria-current={active ? 'page' : undefined}
                >
                  {item.icon({ size: 21 })}
                  {t(item.label)}
                </button>
              )
            })}
            <button
              onClick={() => setMore(true)}
              className={`flex flex-1 flex-col items-center gap-0.5 pb-1.5 pt-2 text-[10.5px] font-semibold transition-colors ${
                !PRIMARY.includes(group) ? 'text-accent' : 'text-ink3 hover:text-ink2'
              }`}
              aria-haspopup="dialog"
            >
              <MoreIcon />
              {t('More')}
            </button>
          </div>
        </nav>
        {more && <MoreSheet group={group} onNavigate={onNavigate} onClose={() => setMore(false)} />}
      </div>
    </div>
  )
}

function MoreIcon() {
  return (
    <svg width={21} height={21} viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      <circle cx={5} cy={12} r={1.8} />
      <circle cx={12} cy={12} r={1.8} />
      <circle cx={19} cy={12} r={1.8} />
    </svg>
  )
}
