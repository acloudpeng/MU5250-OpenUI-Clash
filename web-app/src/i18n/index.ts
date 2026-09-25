// Interface language. English source strings are the keys; zh/ holds the translations.
// Switching language remounts the app (see App.tsx), so t() is read at render time
// without subscribing components to a context.
import { ZH, ZH_MESSAGES } from './zh'

export type Lang = 'zh' | 'en'

const KEY = 'u60.lang'
type Vars = Record<string, string | number>

function stored(): Lang {
  try {
    return localStorage.getItem(KEY) === 'en' ? 'en' : 'zh'
  } catch {
    return 'zh'
  }
}

let current: Lang = stored()

export const getLang = () => current

/** Locale for dates and numbers: Chinese in the Chinese interface, else the browser's. */
export const locale = () => (current === 'zh' ? 'zh-CN' : undefined)

export function applyLang(lang: Lang = current) {
  current = lang
  try {
    localStorage.setItem(KEY, lang)
  } catch {
    // Private mode: the choice lasts for this page only.
  }
  document.documentElement.lang = lang === 'zh' ? 'zh-CN' : 'en'
}

function fill(text: string, vars?: Vars) {
  return vars ? text.replace(/\{(\w+)\}/g, (m, k: string) => (k in vars ? String(vars[k]) : m)) : text
}

/** Translate an interface string. `{name}` placeholders are filled from `vars`. */
export function t(text: string, vars?: Vars): string {
  return fill(current === 'zh' ? (ZH[text] ?? text) : text, vars)
}

/** Mark a string for translation where it is defined; show it with t(). */
export const N_ = (text: string) => text

// Messages from the agent and the device arrive in English. Known ones are translated
// by exact text or by pattern ("{x}" matches any text); unknown ones pass through.
const PATTERNS = Object.entries(ZH_MESSAGES)
  .filter(([en]) => en.includes('{'))
  .map(([en, zh]) => {
    const names: string[] = []
    const source = en
      .split(/(\{\w+\})/)
      .map((part) => {
        const name = /^\{(\w+)\}$/.exec(part)?.[1]
        if (!name) return part.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
        names.push(name)
        return '([\\s\\S]*?)'
      })
      .join('')
    return { re: new RegExp(`^${source}$`), names, zh }
  })

function message(text: string): string {
  const exact = ZH_MESSAGES[text] ?? ZH[text]
  if (exact) return exact
  for (const p of PATTERNS) {
    const m = p.re.exec(text)
    if (m) return fill(p.zh, Object.fromEntries(p.names.map((n, i) => [n, message(m[i + 1])])))
  }
  return text
}

/** Translate a message that came from the agent or the device, when it is a known one. */
export function tm(text: string): string
export function tm(text: string | null | undefined): string | null | undefined
export function tm(text: string | null | undefined) {
  if (!text || current !== 'zh') return text
  const whole = message(text)
  if (whole !== text) return whole
  // Composite errors ("…; WireGuard could not be restored: …") translate part by part.
  return text.includes('; ') ? text.split('; ').map(message).join('；') : text
}
