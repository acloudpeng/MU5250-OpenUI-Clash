const { test } = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const vm = require('node:vm')
const ts = require('typescript')

// The activation-code rules the page applies before the agent checks them again (esim.rs).
function load() {
  const context = { exports: {}, require: () => ({}) }
  const source = fs.readFileSync(path.join(__dirname, '../src/features/modem/qr.ts'), 'utf8')
  vm.runInNewContext(ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText, context)
  return context.exports
}

test('activation codes parse as the agent parses them', () => {
  const { parseActivation } = load()
  assert.deepEqual({ ...parseActivation('LPA:1$rsp.truphone.com$QR-G-5C-1LS-1W1Z9P7') },
    { smdp: 'rsp.truphone.com', matchingId: 'QR-G-5C-1LS-1W1Z9P7', needsConfirmation: false })
  const a = parseActivation(' lpa:1$SMDP.Example.com:443$ABC$1.3.6.1.4.1.31746$1 ')
  assert.equal(a.smdp, 'smdp.example.com:443')
  assert.equal(a.needsConfirmation, true)
  assert.equal(parseActivation('LPA:1$smdp.example').matchingId, '')
  for (const bad of ['', '1$smdp$ABC', 'LPA:2$smdp$ABC', 'LPA:1$$ABC', 'LPA:1$smdp example$ABC',
    'LPA:1$smdp$AB C', 'LPA:1$-smdp$ABC', 'LPA:1$smdp$A$B$C$D']) {
    assert.equal(parseActivation(bad), null, bad)
  }
})

test('manual entries follow the same rules', () => {
  const { validSmdp, validMatchingId } = load()
  assert.equal(validSmdp('rsp.example.com'), true)
  assert.equal(validSmdp('.example'), false)
  assert.equal(validMatchingId(''), true)
  assert.equal(validMatchingId('AB-12'), true)
  assert.equal(validMatchingId('AB 12'), false)
})
