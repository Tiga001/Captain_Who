import assert from 'node:assert/strict'
import test from 'node:test'

import {
  PACKAGED_AGENT_MCP_DRIVE_ASSESSMENT,
  assertNoBackdoorArguments,
  createOfflineFixtureProxy,
  descendantProcesses,
  parseProcessTable,
  validatePackagedProductionEntry
} from './verify-packaged-playwright-startup.mjs'

test('process-table discovery follows only descendants of the exact packaged process', () => {
  const processes = parseProcessTable(`
    10 1 /Applications/MyCopilot.app/Contents/MacOS/MyCopilot
    11 10 /Applications/MyCopilot.app/Contents/Resources/core-server
    12 10 /Applications/MyCopilot.app/Contents/Frameworks/Renderer --type=renderer
    13 12 /Applications/MyCopilot.app/Contents/Frameworks/Utility --type=utility
    20 1 /Applications/Unrelated.app/Contents/MacOS/Unrelated
  `)
  assert.deepEqual(
    descendantProcesses(processes, 10).map((process) => process.pid),
    [11, 12, 13]
  )
})

test('startup arguments reject debugging, sandbox, extension, and web-security backdoors', () => {
  assert.doesNotThrow(() =>
    assertNoBackdoorArguments([
      '--user-data-dir=/tmp/fixture',
      '--disable-background-networking',
      '--proxy-server=http://127.0.0.1:1234'
    ])
  )
  for (const argument of [
    '--remote-debugging-port=0',
    '--remote-debugging-pipe',
    '--inspect=0',
    '--no-sandbox',
    '--disable-web-security',
    '--load-extension=/tmp/fixture'
  ]) {
    assert.throws(() => assertNoBackdoorArguments([argument]), /backdoor_argument/)
  }
})

test('packaged production entry requires the private bridge and rejects a fixture driver hook', () => {
  assert.doesNotThrow(() =>
    validatePackagedProductionEntry('ManagedPlaywrightBridgeHost isTrustedRendererEvent')
  )
  assert.throws(
    () =>
      validatePackagedProductionEntry(
        'ManagedPlaywrightBridgeHost isTrustedRendererEvent MYCOPILOT_PACKAGED_FIXTURE'
      ),
    /production_backdoor/
  )
  assert.throws(
    () => validatePackagedProductionEntry('ManagedPlaywrightBridgeHost'),
    /production_entry_missing/
  )
})

test('loopback fixture is executable while non-fixture proxy requests fail closed', async () => {
  const fixture = await createOfflineFixtureProxy()
  try {
    await fixture.selfCheck()
    const blocked = await fetch(`${fixture.proxyUrl}/not-a-fixture`)
    assert.equal(blocked.status, 502)
    assert.deepEqual(
      fixture.requests.map((request) => ({
        fixture: request.fixture,
        outbound: request.outbound,
        selfCheck: request.selfCheck
      })),
      [
        { fixture: true, outbound: false, selfCheck: true },
        { fixture: false, outbound: true, selfCheck: false }
      ]
    )
  } finally {
    await fixture.close()
  }
})

test('packaged Agent MCP claim remains explicitly pending without a production driver', () => {
  assert.equal(PACKAGED_AGENT_MCP_DRIVE_ASSESSMENT.status, 'pending')
  assert.equal(
    PACKAGED_AGENT_MCP_DRIVE_ASSESSMENT.reasonCode,
    'no_production_external_managed_mcp_driver'
  )
  assert.match(PACKAGED_AGENT_MCP_DRIVE_ASSESSMENT.nearestExecutableEvidence, /vitest/)
  assert.ok(
    PACKAGED_AGENT_MCP_DRIVE_ASSESSMENT.forbiddenShortcuts.includes('remote_debugging_port_or_pipe')
  )
})
