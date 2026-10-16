const { test } = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const vm = require('node:vm')
const ts = require('typescript')
const { check, isProse } = require('./check-i18n.cjs')

// Loads src/i18n with its dictionaries, resolving the relative imports between them.
function i18n(stored) {
  const storage = new Map(stored ? [['u60.lang', stored]] : [])
  const documentElement = { lang: '' }
  const context = {
    localStorage: { getItem: (k) => storage.get(k) ?? null, setItem: (k, v) => storage.set(k, v) },
    document: { documentElement },
  }
  const cache = new Map()
  const load = (file) => {
    if (cache.has(file)) return cache.get(file).exports
    const module = { exports: {} }
    cache.set(file, module)
    const code = ts.transpileModule(fs.readFileSync(file, 'utf8'), {
      compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
    }).outputText
    const require = (spec) => {
      const base = path.resolve(path.dirname(file), spec)
      return load([base + '.ts', path.join(base, 'index.ts')].find((f) => fs.existsSync(f)))
    }
    vm.runInNewContext(`(function (module, exports, require) {${code}\n})`, context)(module, module.exports, require)
    return module.exports
  }
  return { ...load(path.join(__dirname, '../src/i18n/index.ts')), storage, documentElement }
}

test('Chinese is the default and the choice is remembered', () => {
  const zh = i18n()
  assert.equal(zh.getLang(), 'zh')
  assert.equal(zh.t('Sign in'), '登录')
  zh.applyLang('en')
  assert.equal(zh.t('Sign in'), 'Sign in')
  assert.equal(zh.storage.get('u60.lang'), 'en')
  assert.equal(zh.documentElement.lang, 'en')
  assert.equal(i18n('en').getLang(), 'en')
})

test('placeholders are filled in both languages and unknown text passes through', () => {
  const { t, applyLang } = i18n()
  assert.equal(t('Battery low ({percent}%)', { percent: 12 }), '电池电量低（12%）')
  assert.equal(t('Not in the dictionary {x}', { x: 1 }), 'Not in the dictionary 1')
  applyLang('en')
  assert.equal(t('Battery low ({percent}%)', { percent: 12 }), 'Battery low (12%)')
})

test('agent messages translate by exact text, by pattern and part by part', () => {
  const { tm, applyLang } = i18n()
  assert.equal(tm('Timed out reaching the agent'), '连接 Agent 超时')
  assert.equal(tm('request failed (502)'), '请求失败（502）')
  // The captured part is translated too when it is itself a known message.
  assert.equal(tm('Failed to reach the agent at Timed out reaching the agent'), '无法连接 Agent（连接 Agent 超时）')
  assert.equal(tm('Timed out reaching the agent; request failed (500)'), '连接 Agent 超时；请求失败（500）')
  assert.equal(tm('something the agent never said'), 'something the agent never said')
  assert.equal(tm(null), null)
  applyLang('en')
  assert.equal(tm('Timed out reaching the agent'), 'Timed out reaching the agent')
})

test('core job errors translate with their reasons and inside rollback composites', () => {
  const { tm } = i18n()
  assert.equal(tm('Download failed: cannot resolve proxy (curl 5)'), '下载失败：无法解析代理地址（curl 5）')
  assert.equal(tm('New core and previous core failed to start: Clash core exited after starting; Clash core did not answer within 15 s'),
    '新内核和之前的内核均启动失败：Clash 内核启动后退出；Clash 内核未在 15 秒内响应')
  assert.equal(tm('New geo data failed to load; previous data restored: Clash profile rejected: bad rule'),
    '新 Geo 数据加载失败，已恢复之前的数据：Clash 配置未通过检查：bad rule')
})

test('the checker tells interface text from code', () => {
  assert.ok(isProse('Unsaved changes'))
  assert.ok(isProse('Save'))
  assert.ok(isProse('connected', true))
  for (const code of ['flex items-center gap-2', 'clash-clients', '/api/clash', 'selected', 'RSRP', '5 GHz', 'MB']) {
    assert.ok(!isProse(code), code)
  }
})

test('all interface text is translated and the dictionary is consistent', () => {
  assert.deepEqual(check(), [])
})

test('Wi-Fi relay messages translate, including composed ones', () => {
  const { tm } = i18n()
  assert.equal(tm('The upstream network 192.168.0.0/24 overlaps the LAN; change the LAN address'), '上游网络 192.168.0.0/24 与本机局域网网段冲突，请修改局域网地址')
  assert.equal(tm('Wi-Fi uplink network operation failed: iptables: Resource temporarily unavailable'), 'Wi-Fi 中继 网络操作失败：iptables: Resource temporarily unavailable')
  assert.equal(tm('At most 8 networks can be saved'), '最多只能保存 8 个网络')
  assert.equal(tm('Cannot start /usr/sbin/wpa_supplicant'), '无法启动 /usr/sbin/wpa_supplicant')
  assert.equal(tm('Cannot start lpac: busy'), '无法启动 lpac：busy')
})
