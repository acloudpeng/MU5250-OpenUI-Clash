// Chinese translations, one file per area. Keys are the English source strings.
// tools/check-i18n.cjs checks every t()/N_() string has an entry and entries agree.
import agent from './agent'
import app from './app'
import devui from './devui'
import home from './home'
import modem from './modem'
import network from './network'
import proxy from './proxy'
import signal from './signal'
import system from './system'

export type Dict = Record<string, string>

export const ZH: Dict = { ...app, ...home, ...signal, ...network, ...proxy, ...modem, ...devui, ...system }
export const ZH_MESSAGES: Dict = agent
