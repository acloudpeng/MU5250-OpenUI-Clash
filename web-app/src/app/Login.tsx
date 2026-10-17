import { useState } from 'react'
import { login, setToken } from '../data/client'
import { ISignal } from '../icons'
import { Button } from '../ui/controls'
import { Segmented } from '../ui/controls'
import { t, tm, type Lang } from '../i18n'
import { LangButton } from './Shell'

function isMobilePinClient() {
  const ua = navigator.userAgent
  return (
    /Android|webOS|iPhone|iPad|iPod|BlackBerry|IEMobile|Opera Mini|Mobile/i.test(ua) ||
    (navigator.maxTouchPoints > 1 && /Macintosh/i.test(ua))
  )
}

export default function Login({ onAuthed, lang, onToggleLang }: { onAuthed: () => void; lang: Lang; onToggleLang: () => void }) {
  const mobile = isMobilePinClient()
  const [mode, setMode] = useState<'pin' | 'password'>(mobile ? 'pin' : 'password')
  const [pw, setPw] = useState('')
  const [pin, setPin] = useState('')
  const [err, setErr] = useState('')
  const [busy, setBusy] = useState(false)

  async function submit(e: React.FormEvent) {
    e.preventDefault()
    setBusy(true)
    setErr('')
    try {
      const { token } = await login(mode === 'pin' ? { pin } : { password: pw })
      setToken(token)
      onAuthed()
    } catch (error) {
      setErr(error instanceof Error ? tm(error.message) : t('Sign in failed'))
    } finally {
      setBusy(false)
    }
  }

  const canSubmit = mode === 'pin' ? pin.length === 6 : pw.length > 0

  return (
    <div className="relative flex min-h-full items-center justify-center bg-bg p-6">
      <div className="absolute right-4 top-4">
        <LangButton lang={lang} onToggle={onToggleLang} />
      </div>
      <div className="w-full max-w-sm">
        <div className="mb-6 flex flex-col items-center text-center">
          <div className="mb-3 flex h-12 w-12 items-center justify-center rounded-xl bg-accent text-white">
            <ISignal size={24} />
          </div>
          <h1 className="text-lg font-bold text-ink">ZTE U60 Pro</h1>
          <p className="mt-0.5 text-[13px] text-ink2">{t('Advanced admin')}</p>
        </div>

        <form
          onSubmit={submit}
          className="space-y-4 rounded-xl border border-line/8 bg-surface p-5"
        >
          {mobile && (
            <div className="flex justify-center">
              <Segmented
                options={[
                  { value: 'pin', label: 'PIN' },
                  { value: 'password', label: t('Password') },
                ]}
                value={mode}
                onChange={(m) => {
                  setMode(m)
                  setErr('')
                }}
              />
            </div>
          )}

          {mode === 'pin' ? (
            <input
              type="password"
              inputMode="numeric"
              pattern="[0-9]*"
              maxLength={6}
              value={pin}
              onChange={(e) => setPin(e.target.value.replace(/\D/g, '').slice(0, 6))}
              className="tnum h-12 w-full rounded-lg border border-line/12 bg-surface2/50 pl-[0.4em] text-center text-2xl font-bold tracking-[0.4em] text-ink outline-none transition-colors placeholder:text-ink3 focus:border-accent/60"
              placeholder="••••••"
              autoFocus
              autoComplete="one-time-code"
              enterKeyHint="done"
              aria-label="PIN"
            />
          ) : (
            <input
              type="password"
              value={pw}
              onChange={(e) => setPw(e.target.value)}
              className="h-11 w-full rounded-lg border border-line/12 bg-surface2/50 px-3.5 text-sm text-ink outline-none transition-colors placeholder:text-ink3 focus:border-accent/60"
              placeholder={t('Agent password')}
              autoFocus
              autoComplete="current-password"
              aria-label={t('Agent password')}
            />
          )}

          {err && <p className="text-xs font-medium text-danger">{err}</p>}

          <Button type="submit" variant="primary" loading={busy} disabled={!canSubmit} className="w-full !h-11">
            {t('Sign in')}
          </Button>
        </form>
      </div>
    </div>
  )
}
