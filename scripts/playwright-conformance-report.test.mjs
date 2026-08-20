import assert from 'node:assert/strict'
import { cp, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

import {
  PLAYWRIGHT_CONFORMANCE_REPORT_PATH,
  buildPlaywrightConformanceReport,
  checkPlaywrightConformanceReport,
  serializePlaywrightConformanceReport
} from './playwright-conformance-report.mjs'

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')

test('derives every tool row from the frozen inputs and keeps all policy states explicit', async () => {
  const report = await buildPlaywrightConformanceReport()

  assert.equal(report.package.name, '@playwright/mcp')
  assert.equal(report.package.version, '0.0.79')
  assert.equal(report.classification.upstreamToolCount, 69)
  assert.equal(report.classification.policyToolCount, 69)
  assert.equal(report.classification.exposedToolCount, 61)
  assert.equal(report.classification.hiddenToolCount, 8)
  assert.deepEqual(report.classification.handlingModes, {
    pass_through: 25,
    host_adapted: 8,
    approval_required: 21,
    artifact_managed: 7,
    sandboxed: 1,
    unsupported: 7
  })
  assert.equal(new Set(report.tools.map((tool) => tool.rawName)).size, 69)
  assert.equal(report.tools.filter((tool) => tool.handlingMode === 'approval_required').length, 21)
  const unsafe = report.tools.find((tool) => tool.rawName === 'browser_run_code_unsafe')
  assert.equal(unsafe.handlingMode, 'sandboxed')
  assert.equal(unsafe.exposed, false)
  assert.equal(unsafe.availability, 'model_hidden')
  assert.equal(unsafe.behaviorContract, 'unavailable_until_real_process_sandbox')
  const pdf = report.tools.find((tool) => tool.rawName === 'browser_pdf_save')
  assert.equal(pdf.exposed, true)
  assert.equal(pdf.availability, 'model_visible_typed_unavailable')
  assert.equal(pdf.behaviorContract, 'typed_platform_unavailable')
  assert.equal(report.platformCapabilityDifferences.browser_pdf_save.status, 'typed_unavailable')
  assert.equal(
    report.platformCapabilityDifferences.browser_pdf_save.toolResult.code,
    'browser.pdf_unavailable'
  )
  assert.equal(report.platformCapabilityDifferences.browser_pdf_save.artifactPublished, false)
  assert.equal(report.knownBehaviorGaps.popupOpenerSemantics.status, 'not_native_parity')
  assert.equal(report.knownBehaviorGaps.popupOpenerSemantics.nativeWindowProxy, false)
  assert.equal(report.knownBehaviorGaps.popupOpenerSemantics.openerPostMessage, false)
  assert.equal(report.knownBehaviorGaps.workerTargets.serviceWorker.status, 'not_admitted')
  assert.equal(report.knownBehaviorGaps.workerTargets.sharedWorker.electronE2e, false)
  assert.equal(
    report.knownBehaviorGaps.workerTargets.boundary,
    'managed_page_frame_and_dedicated_worker_targets_only'
  )
  assert.equal(report.security.persistenceLeakBudget.cookieValues, 0)
  assert.equal(report.platforms.find((entry) => entry.platform === 'darwin').status, 'evidence')
  assert.equal(
    report.releaseGates.find((gate) => gate.id === 'packaged_application_offline_e2e').status,
    'pending'
  )
  assert.equal(
    report.releaseGates.find((gate) => gate.id === 'deterministic_round_3_host_stress').status,
    'evidence'
  )
  assert.equal(
    report.releaseGates.find((gate) => gate.id === 'packaged_application_offline_startup').status,
    'evidence'
  )
  assert.equal(
    report.stressEvidence.covered.find(
      (entry) => entry.scenario === 'managed_host_read_tool_dispatch'
    ).iterations,
    500
  )
  const concurrentRead = report.stressEvidence.covered.find(
    (entry) => entry.scenario === 'concurrent_read_admission_attempts'
  )
  assert.equal(concurrentRead.iterations, 32)
  assert.equal(concurrentRead.firstWaveAdmitted, 8)
  assert.equal(concurrentRead.firstWaveRejectedBusy, 24)
  assert.equal(concurrentRead.completedAfterBoundedRetry, 32)
  assert.equal(concurrentRead.admissionWaves, 4)
  for (const scenario of [
    'tab_create_close',
    'route_unroute',
    'file_selection_and_consume',
    'protected_storage_export_and_revoke'
  ]) {
    assert.equal(
      report.stressEvidence.covered.find((entry) => entry.scenario === scenario).iterations,
      100
    )
  }
  for (const scenario of ['target_crash_recovery', 'relay_generation_recovery']) {
    assert.equal(
      report.stressEvidence.covered.find((entry) => entry.scenario === scenario).iterations,
      25
    )
  }
  assert.equal(report.packagedApplication.status, 'evidence')
  assert.equal(
    report.packagedApplication.observedEvidence.scope,
    'unpacked_macos_process_startup_only'
  )
  assert.equal(report.packagedApplication.agentMcpLocalFixtureE2e.status, 'pending')
  assert.equal(
    report.packagedApplication.agentMcpLocalFixtureE2e.reasonCode,
    'no_production_external_managed_mcp_driver'
  )
  assert.equal(report.packagedApplication.agentMcpLocalFixtureE2e.localFixtureAgentMcpRequests, 0)
  assert.deepEqual(
    report.sensitiveResourceScope.tools.map((entry) => [
      entry.tool,
      entry.scope,
      entry.available.length,
      Boolean(entry.exactEffectEvidence)
    ]),
    [
      ['browser_evaluate', 'managed_surface', 4, true],
      ['browser_drop', 'managed_surface', 5, true],
      ['browser_file_upload', 'managed_surface', 2, true],
      ['browser_network_request', 'managed_surface', 2, true],
      ['browser_cookie_*', 'managed_browser_profile', 2, false],
      ['browser_storage_state/browser_set_storage_state', 'managed_browser_profile', 2, false]
    ]
  )
})

test('matches the committed deterministic report byte for byte', async () => {
  await checkPlaywrightConformanceReport()
  assert.equal(
    await readFile(PLAYWRIGHT_CONFORMANCE_REPORT_PATH, 'utf8'),
    await serializePlaywrightConformanceReport()
  )
})

test('fails closed if one policy row no longer matches the frozen upstream schema digest', async () => {
  const temporaryRoot = await mkdtemp(join(tmpdir(), 'mycopilot-conformance-'))
  try {
    for (const relativePath of [
      'crates/core-server/resources/playwright-upstream-catalog-0.0.79.json',
      'crates/core-server/resources/playwright-browser-manifest-v1.json',
      'src/main/mcp/ManagedPlaywrightMcpHost.ts',
      'src/main/browser/ElectronGuestCdpTransport.ts',
      'src/main/browser/BrowserArtifactBroker.ts',
      'src/main/browser/BrowserFileBroker.ts',
      'src/main/browser/BrowserTargetBroker.ts'
    ]) {
      const target = join(temporaryRoot, relativePath)
      await mkdir(dirname(target), { recursive: true })
      await cp(join(repositoryRoot, relativePath), target, { recursive: true })
    }
    const manifestPath = join(
      temporaryRoot,
      'crates/core-server/resources/playwright-browser-manifest-v1.json'
    )
    const manifest = JSON.parse(await readFile(manifestPath, 'utf8'))
    manifest.tools[0].upstreamSchemaDigest = `sha256:${'0'.repeat(64)}`
    await writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`, 'utf8')

    await assert.rejects(
      buildPlaywrightConformanceReport({ repositoryRoot: temporaryRoot }),
      /invalid_policy_digest|policy_catalog_drift/
    )
  } finally {
    await rm(temporaryRoot, { recursive: true, force: true })
  }
})

test('fails closed if an upstream schema changes without recomputing its digests', async () => {
  const temporaryRoot = await mkdtemp(join(tmpdir(), 'mycopilot-conformance-schema-'))
  try {
    for (const relativePath of [
      'crates/core-server/resources/playwright-upstream-catalog-0.0.79.json',
      'crates/core-server/resources/playwright-browser-manifest-v1.json',
      'src/main/mcp/ManagedPlaywrightMcpHost.ts',
      'src/main/browser/ElectronGuestCdpTransport.ts',
      'src/main/browser/BrowserArtifactBroker.ts',
      'src/main/browser/BrowserFileBroker.ts',
      'src/main/browser/BrowserTargetBroker.ts'
    ]) {
      const target = join(temporaryRoot, relativePath)
      await mkdir(dirname(target), { recursive: true })
      await cp(join(repositoryRoot, relativePath), target, { recursive: true })
    }
    const catalogPath = join(
      temporaryRoot,
      'crates/core-server/resources/playwright-upstream-catalog-0.0.79.json'
    )
    const catalog = JSON.parse(await readFile(catalogPath, 'utf8'))
    catalog.tools[0].inputSchema.properties.unreviewed = { type: 'string' }
    await writeFile(catalogPath, `${JSON.stringify(catalog, null, 2)}\n`, 'utf8')

    await assert.rejects(
      buildPlaywrightConformanceReport({ repositoryRoot: temporaryRoot }),
      /invalid_upstream_catalog_digest|invalid_upstream_tool_digest/
    )
  } finally {
    await rm(temporaryRoot, { recursive: true, force: true })
  }
})
