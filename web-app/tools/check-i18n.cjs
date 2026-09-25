#!/usr/bin/env node
// Finds interface text that bypasses translation and checks the Chinese dictionary.
//
// Reported:
//   - JSX text and prose string literals outside t()/N_()/tm() (English UI text);
//   - t()/N_() strings without a Chinese entry, or whose {placeholders} differ;
//   - the same English key translated differently in two area files;
//   - dictionary entries no longer used by any t()/N_() call (except agent messages).
// A literal on a line containing "i18n-ignore" (or the line after one) is skipped.
const fs = require('node:fs')
const path = require('node:path')
const ts = require('typescript')

const SRC = path.join(__dirname, '../src')
const SKIP_FILES = new Set(['types.ts', 'icons.tsx', 'main.tsx', 'vite-env.d.ts'])
const SKIP_DIRS = new Set(['i18n'])
// JSX attributes that never hold interface text.
const CODE_ATTRS = new Set([
  'className', 'key', 'type', 'id', 'name', 'value', 'defaultValue', 'href', 'role', 'inputMode', 'autoComplete',
  'enterKeyHint', 'pattern', 'variant', 'tone', 'size', 'target', 'rel', 'method', 'htmlFor', 'd', 'viewBox',
  'fill', 'stroke', 'strokeWidth', 'strokeLinecap', 'strokeLinejoin', 'src', 'dir', 'lang', 'accept', 'form',
  'spellCheck', 'aria-hidden', 'aria-modal', 'aria-live', 'aria-current', 'aria-expanded', 'autoCapitalize', 'autoCorrect', 'step', 'min', 'max', 'wrap', 'align', 'mode', 'kind', 'active',
])
// Object keys that never hold interface text.
const CODE_KEYS = new Set([
  'id', 'key', 'value', 'mode', 'kind', 'level', 'tone', 'variant', 'className', 'cls', 'type', 'method', 'path',
  'url', 'color', 'icon', 'state', 'action', 'unit', 'format', 'field', 'band', 'tech', 'cmd', 'command',
])
// Calls whose string arguments are code, not interface text.
const CODE_CALLS = new Set([
  't', 'N_', 'tm', 'usePoll', 'get', 'post', 'put', 'req', 'fetch', 'getItem', 'setItem', 'removeItem', 'log',
  'warn', 'error', 'debug', 'info', 'querySelector', 'getElementById', 'addEventListener', 'removeEventListener',
  'dispatchEvent', 'includes', 'startsWith', 'endsWith', 'split', 'join', 'replace', 'replaceAll', 'match', 'test',
  'padStart', 'padEnd', 'toLocaleString', 'toLocaleDateString', 'toLocaleTimeString', 'setAttribute', 'getAttribute',
  'createElement', 'indexOf', 'lastIndexOf', 'Event', 'CustomEvent', 'RegExp', 'Intl', 'DateTimeFormat', 'NumberFormat',
  'localeCompare', 'toFixed', 'readCsv', 'toUpperCase', 'toLowerCase', 'trim', 'has', 'delete', 'set', 'matchMedia',
  'download', 'Blob', 'encodeURIComponent', 'encodeURI', 'atob', 'btoa', 'parseInt', 'parseFloat', 'Number', 'at',
])
// Words that stay the same in Chinese: names, units and protocol terms.
const TERMS = new Set(`
  Clash Meta mihomo FlClash WireGuard OpenUI OpenWrt ZTE Pro Wi Fi WiFi DevUI GitHub Android iPhone Linux Qualcomm Band
  dBm dB MHz GHz KHz kHz Mbps Kbps Gbps bps ms KB MB GB TB KiB MiB GiB Hz mAh mA mW mV rpm pH ms px ok
  ubus uci iptables nftables adb ssh dropbear ttyd tcp udp http https ipv ip nr lte sms at ttl apn usb ncm rndis
  ecm mbim qmi imei imsi iccid msisdn pin puk sim esim vpn lan wan dns dhcp mac mtu ssid wpa psk sae eth wlan bssid
  tun yaml
`.trim().split(/\s+/).map((w) => w.toLowerCase()))

function files(dir) {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = path.join(dir, e.name)
    if (e.isDirectory()) return SKIP_DIRS.has(e.name) ? [] : files(p)
    return /\.(ts|tsx)$/.test(e.name) && !SKIP_FILES.has(e.name) && !e.name.endsWith('.d.ts') ? [p] : []
  })
}

function isClassList(s) {
  const tokens = s.trim().split(/\s+/)
  const styled = tokens.filter((w) => /[-:[/]/.test(w)).length
  return tokens.every((w) => /^[!a-z0-9:\-[\]/.%#()_&>*=,'"@~+]+$/.test(w)) && styled * 2 >= tokens.length
}

// True when the text reads as English words someone would need translated.
function isProse(raw, loose) {
  const s = raw.replace(/\$\{[^}]*\}|\{\w+\}|&[a-z]+;/g, ' ').trim()
  if (!s || isClassList(s)) return false
  if (/^(https?:|\/|\.\/|#|[\w-]+\/[\w-]+$)/.test(s)) return false
  const words = (s.match(/[A-Za-z][A-Za-z']*/g) ?? []).filter((w) => !TERMS.has(w.toLowerCase()) && !/^[A-Z0-9]+s?$/.test(w))
  if (words.length === 0) return false
  if (loose) return words.some((w) => w.length >= 2)
  // Outside JSX: a sentence (space and a word) or a Capitalized label.
  return (/\s/.test(s) && words.some((w) => /[a-z]{2,}/.test(w))) || /^[A-Z][a-z]+/.test(words[0]) && s.startsWith(words[0])
}

function calleeName(call) {
  const e = call.expression
  if (ts.isIdentifier(e)) return e.text
  if (ts.isPropertyAccessExpression(e)) return e.name.text
  return ''
}

function literalText(node) {
  if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) return node.text
  if (ts.isTemplateExpression(node)) return node.head.text + node.templateSpans.map((s) => ' ${} ' + s.literal.text).join('')
  return null
}

function scan(file) {
  const text = fs.readFileSync(file, 'utf8')
  const sf = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, file.endsWith('.tsx') ? ts.ScriptKind.TSX : ts.ScriptKind.TS)
  const lines = text.split('\n')
  const rel = path.relative(path.join(__dirname, '..'), file)
  const untranslated = []
  const keys = []
  const ignored = (node) => {
    const line = sf.getLineAndCharacterOfPosition(node.getStart(sf)).line
    return /i18n-ignore/.test(lines[line]) || (line > 0 && /i18n-ignore/.test(lines[line - 1]))
  }
  const report = (node, s) => {
    if (ignored(node)) return
    const line = sf.getLineAndCharacterOfPosition(node.getStart(sf)).line + 1
    untranslated.push(`${rel}:${line}: ${JSON.stringify(s.trim().replace(/\s+/g, ' ').slice(0, 90))}`)
  }
  // Is this literal in a position where it is code rather than interface text?
  function codePosition(node) {
    let child = node
    for (let p = node.parent; p; child = p, p = p.parent) {
      if (ts.isImportDeclaration(p) || ts.isExportDeclaration(p) || ts.isLiteralTypeNode(p) || ts.isTypeNode(p)) return true
      if (ts.isCallExpression(p) || ts.isNewExpression(p)) return p.expression !== child && CODE_CALLS.has(calleeName(p))
      if (ts.isBinaryExpression(p) && ['===', '!==', '==', '!=', 'in', 'instanceof'].includes(ts.tokenToString(p.operatorToken.kind))) return true
      if (ts.isCaseClause(p) && p.expression === child) return true
      if (ts.isElementAccessExpression(p) && p.argumentExpression === child) return true
      if (ts.isPropertyAssignment(p)) {
        if (p.name === child) return true
        const key = p.name.getText(sf).replace(/['"]/g, '')
        return CODE_KEYS.has(key)
      }
      if (ts.isJsxAttribute(p)) return CODE_ATTRS.has(p.name.getText(sf)) || /^data-/.test(p.name.getText(sf))
      if (ts.isJsxExpression(p) || ts.isJsxElement(p) || ts.isVariableDeclaration(p) || ts.isReturnStatement(p) ||
          ts.isArrowFunction(p) || ts.isBlock(p)) return false
    }
    return false
  }
  function visit(node) {
    if (ts.isCallExpression(node) && ['t', 'N_'].includes(calleeName(node)) && ts.isIdentifier(node.expression)) {
      const arg = node.arguments[0]
      // t(variable) is fine: the strings it can hold are marked with N_() where they are defined.
      if (arg && (ts.isStringLiteral(arg) || ts.isNoSubstitutionTemplateLiteral(arg))) keys.push({ key: arg.text, where: rel })
    }
    if (ts.isJsxText(node)) {
      if (isProse(node.text, true)) report(node, node.text)
    } else {
      const s = literalText(node)
      if (s != null) {
        const jsxAttr = ts.isJsxAttribute(node.parent)
        if (!codePosition(node) && isProse(s, jsxAttr)) report(node, s)
        if (ts.isTemplateExpression(node)) node.templateSpans.forEach((span) => visit(span.expression))
        return
      }
    }
    ts.forEachChild(node, visit)
  }
  visit(sf)
  return { untranslated, keys }
}

function loadDicts() {
  const dir = path.join(SRC, 'i18n/zh')
  const dicts = {}
  for (const name of fs.readdirSync(dir)) {
    if (name === 'index.ts') continue
    const out = ts.transpileModule(fs.readFileSync(path.join(dir, name), 'utf8'), {
      compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
    }).outputText
    const module = { exports: {} }
    new Function('module', 'exports', 'require', out)(module, module.exports, () => ({}))
    dicts[name.replace(/\.ts$/, '')] = module.exports.default
  }
  return dicts
}

const placeholders = (s) => [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort().join(',')

function check() {
  const problems = []
  const used = new Map()
  for (const file of files(SRC)) {
    const { untranslated, keys } = scan(file)
    problems.push(...untranslated.map((p) => `untranslated  ${p}`))
    for (const k of keys) used.set(k.key, k.where)
  }
  const dicts = loadDicts()
  const merged = new Map()
  for (const [area, dict] of Object.entries(dicts)) {
    for (const [en, zh] of Object.entries(dict)) {
      if (typeof zh !== 'string' || !zh.trim()) problems.push(`empty         zh/${area}.ts: ${JSON.stringify(en)}`)
      else if (placeholders(en) !== placeholders(zh)) problems.push(`placeholders  zh/${area}.ts: ${JSON.stringify(en)} -> ${JSON.stringify(zh)}`)
      if (area === 'agent') continue
      const prior = merged.get(en)
      if (prior && prior.zh !== zh) problems.push(`conflict      ${JSON.stringify(en)}: zh/${prior.area}.ts ${JSON.stringify(prior.zh)} vs zh/${area}.ts ${JSON.stringify(zh)}`)
      if (!prior) merged.set(en, { zh, area })
      if (!used.has(en)) problems.push(`unused        zh/${area}.ts: ${JSON.stringify(en)}`)
    }
  }
  for (const [key, where] of used) {
    if (!merged.has(key)) problems.push(`missing       ${where}: ${JSON.stringify(key)}`)
  }
  return problems
}

module.exports = { check, isProse }

if (require.main === module) {
  const problems = check()
  const only = process.argv[2]
  const shown = only ? problems.filter((p) => p.includes(only)) : problems
  console.log(shown.join('\n'))
  console.log(`${shown.length} problem(s)`)
  process.exit(shown.length ? 1 : 0)
}
