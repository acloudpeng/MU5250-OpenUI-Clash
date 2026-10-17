import { Fragment, lazy, useEffect, useState } from 'react'
import { AUTH_EXPIRED_EVENT, clearToken, hasToken } from './data/client'
import { HomeProvider } from './app/HomeContext'
import Login from './app/Login'
import Shell, { type Group } from './app/Shell'
import { useTheme } from './app/theme'
import { ConfirmHost, Toaster } from './ui/feedback'
import { applyLang, getLang, type Lang } from './i18n'
import { api } from './data/api'

const HomePage = lazy(() => import('./features/home/HomePage'))
const SignalGroup = lazy(() => import('./features/signal/SignalGroup'))
const NetworkGroup = lazy(() => import('./features/network/NetworkGroup'))
const ProxyPage = lazy(() => import('./features/proxy/ProxyPage'))
const DevUIPage = lazy(() => import('./features/devui/DevUIPage'))
const ModemGroup = lazy(() => import('./features/modem/ModemGroup'))
const SystemGroup = lazy(() => import('./features/system/SystemGroup'))

export default function App() {
  const [authed, setAuthed] = useState(hasToken())
  const [group, setGroup] = useState<Group>('home')
  const { theme, toggle } = useTheme()
  const [lang, setLang] = useState<Lang>(getLang())
  const toggleLang = () => {
    const next = lang === 'zh' ? 'en' : 'zh'
    applyLang(next)
    setLang(next)
  }

  // The device screen (DevUI) follows the panel language: sent after sign-in and on
  // every switch. An agent without the endpoint simply keeps its language.
  useEffect(() => {
    if (authed) api.screenLanguageSet(lang).catch(() => {})
  }, [authed, lang])

  useEffect(() => {
    const onAuthExpired = () => setAuthed(false)
    window.addEventListener(AUTH_EXPIRED_EVENT, onAuthExpired)
    return () => window.removeEventListener(AUTH_EXPIRED_EVENT, onAuthExpired)
  }, [])

  if (!authed) {
    return (
      <>
        <Login key={lang} onAuthed={() => setAuthed(true)} lang={lang} onToggleLang={toggleLang} />
        <Toaster />
      </>
    )
  }

  return (
    <>
      {/* Remounting on a language switch re-renders every t() call; polled data is cached. */}
      <Fragment key={lang}>
      <HomeProvider fast={group === 'home' || group === 'signal'}>
        <Shell group={group} onNavigate={setGroup} theme={theme} onToggleTheme={toggle} lang={lang} onToggleLang={toggleLang}>
          {group === 'home' && <HomePage onNavigate={setGroup} />}
          {group === 'signal' && <SignalGroup />}
          {group === 'network' && <NetworkGroup />}
          {group === 'proxy' && <ProxyPage />}
          {group === 'devui' && <DevUIPage />}
          {group === 'modem' && <ModemGroup />}
          {group === 'system' && (
            <SystemGroup
              onLogout={() => {
                clearToken()
                setAuthed(false)
              }}
            />
          )}
        </Shell>
      </HomeProvider>
      </Fragment>
      <Toaster />
      <ConfirmHost />
    </>
  )
}
