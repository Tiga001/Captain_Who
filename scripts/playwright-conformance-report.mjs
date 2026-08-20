/* eslint-disable @typescript-eslint/explicit-function-return-type */
import { createHash } from 'node:crypto'
import { readFile, writeFile } from 'node:fs/promises'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { format, resolveConfig } from 'prettier'

const REPOSITORY_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
export const PLAYWRIGHT_CONFORMANCE_REPORT_PATH = resolve(
  REPOSITORY_ROOT,
  'crates/core-server/resources/playwright-conformance-report.json'
)
const UPSTREAM_CATALOG_PATH = resolve(
  REPOSITORY_ROOT,
  'crates/core-server/resources/playwright-upstream-catalog-0.0.79.json'
)
const POLICY_MANIFEST_PATH = resolve(
  REPOSITORY_ROOT,
  'crates/core-server/resources/playwright-browser-manifest-v1.json'
)

const HANDLING_MODES = new Set([
  'pass_through',
  'host_adapted',
  'approval_required',
  'artifact_managed',
  'sandboxed',
  'unsupported'
])
const EXPOSED_HANDLING_MODES = new Set([
  'pass_through',
  'host_adapted',
  'approval_required',
  'artifact_managed'
])
const TOOL_NAME = /^[A-Za-z0-9_-]{1,64}$/u
const SHA256_DIGEST = /^sha256:[0-9a-f]{64}$/u
const HOST_CALL_REASON_SCHEMA = Object.freeze({
  type: 'string',
  description: 'Brief reason for using this browser tool.',
  minLength: 1,
  maxLength: 512
})

const SCHEMA_PARITY_KEYWORDS = Object.freeze([
  'type',
  'required',
  'properties',
  'items',
  'enum',
  'const',
  'additionalProperties',
  'oneOf',
  'anyOf',
  'allOf',
  'prefixItems',
  'minimum',
  'maximum',
  'minLength',
  'maxLength',
  'description'
])

const RELEASE_GATES = Object.freeze([
  {
    id: 'catalog_lock_and_policy_bijection',
    status: 'contract',
    scope: 'All fixed-package tools have exactly one reviewed policy row and matching digests.',
    evidence: [
      'src/main/mcp/managedPlaywrightCatalog.test.ts > managed Playwright fixed Catalog',
      'scripts/playwright-conformance-report.test.mjs > derives every tool row from the frozen inputs'
    ]
  },
  {
    id: 'recursive_schema_and_provider_payload_parity',
    status: 'evidence',
    scope: 'Upstream schema, Host overlay, Rust registry, OpenAI payload, and Anthropic payload.',
    execution: 'not_recorded_in_source_control',
    evidence: [
      'src/main/mcp/managedPlaywrightCatalog.test.ts',
      'packages/protocol/src/mcp/managedPlaywrightBridge.test.ts',
      'crates/core-server/src/application/mcp/playwright_manifest.rs'
    ]
  },
  {
    id: 'raw_official_and_external_reference',
    status: 'evidence',
    scope: 'Pinned createConnection and pinned stdio Server use the same repository-local fixture.',
    execution: 'observed_on_2026-08-20',
    observedEvidence: {
      platform: 'darwin-arm64',
      command: 'pnpm exec vitest run --project unit src/main/mcp/managedPlaywrightCatalog.test.ts',
      result: 'passed',
      tests: 8
    },
    evidence: [
      'src/main/mcp/managedPlaywrightCatalog.test.ts > matches RawOfficial and ExternalReference behavior on one isolated local workflow'
    ]
  },
  {
    id: 'builtin_managed_electron_guest',
    status: 'evidence',
    scope: 'Private Broker, managed guest, BrowserSurface lifecycle, and fixed MCP invocation.',
    execution: 'observed_on_2026-08-20',
    observedEvidence: {
      platform: 'darwin-arm64',
      command:
        'pnpm exec vitest run --project unit src/main/core/managedPlaywrightBridge.electron.test.ts',
      result: 'passed',
      tests: 1
    },
    evidence: [
      'src/main/core/browserTargetBroker.electron.test.ts',
      'src/main/browser/fixtures/managedPlaywrightBridge.electron.ts'
    ]
  },
  {
    id: 'iframe_input_parity',
    status: 'evidence',
    scope:
      'Same-origin, OOPIF, sibling, nested, blank contenteditable, textarea, input, and Shadow DOM input delivery.',
    execution: 'observed_on_2026-08-20',
    observedEvidence: {
      platform: 'darwin-arm64',
      command:
        'pnpm exec vitest run --project unit src/main/core/browserTargetBroker.electron.test.ts',
      result: 'passed',
      tests: 1
    },
    evidence: [
      'src/main/browser/fixtures/browserTargetBroker.electron.ts',
      'src/main/core/browserTargetBroker.electron.test.ts',
      'src/main/browser/fixtures/managedPlaywrightBridge.electron.ts'
    ]
  },
  {
    id: 'sensitive_tool_internal_approval',
    status: 'evidence',
    scope: 'Approve-success, approve-failure, reject, reject-with-reason, races, and expiry.',
    execution: 'not_recorded_in_source_control',
    evidence: [
      'crates/core-server/src/application/mcp/builtin_capability_runtime.rs',
      'src/renderer/src/app/__tests__/BuiltinMcpToolApprovalCard.browser.test.tsx'
    ]
  },
  {
    id: 'file_artifact_and_secret_boundaries',
    status: 'evidence',
    scope:
      'Opaque file authority, protected artifacts, bounded output, and safe durable projections.',
    execution: 'not_recorded_in_source_control',
    evidence: [
      'src/main/core/browserFileBroker.test.ts',
      'src/main/core/browserArtifactBroker.test.ts',
      'crates/core/src/runtime/checkpoint/tests/creation_and_mcp.rs'
    ]
  },
  {
    id: 'complete_69_tool_behavior_matrix',
    status: 'pending',
    scope:
      'Success, invalid input, cancellation, over-limit, and target-close for every model-visible tool.',
    reason: 'No single automated matrix currently proves all five cases for every visible tool.'
  },
  {
    id: 'deterministic_round_3_host_stress',
    status: 'evidence',
    scope:
      'Managed Host dispatch, bounded concurrent admission, tab/route/file/storage cleanup, Target crash recovery, and relay generation recovery.',
    execution: 'observed_on_2026-08-20',
    observedEvidence: {
      platform: 'darwin-arm64',
      command: 'pnpm test:playwright-round3-stress',
      result: 'passed',
      tests: 5
    },
    evidence: ['src/main/mcp/managedPlaywrightRound3Stress.test.ts']
  },
  {
    id: 'complete_round_3_stress_matrix',
    status: 'pending',
    scope:
      'The deterministic Host gate plus repeated real-Electron browser work and RSS assertions.',
    reason:
      'The deterministic Host gate is observed, but 500 real guest actions, 100 real download cycles, repeated real renderer/server crashes, and an RSS-growth gate remain unproved.'
  },
  {
    id: 'packaged_application_offline_startup',
    status: 'evidence',
    scope:
      'Start the real unpacked macOS bundle with temporary appData, a loopback deny proxy, and a repository-local fixture.',
    execution: 'observed_on_2026-08-20',
    observedEvidence: {
      platform: 'darwin-arm64',
      command: 'pnpm verify:playwright-packaged-startup',
      result: 'passed',
      sourceFreshness: 'not_established_gate_verifies_the_supplied_bundle_only',
      coreServerProcesses: 1,
      rendererProcesses: 1,
      proxiedOutboundAttempts: 0,
      localFixtureSelfCheck: 'passed',
      localFixtureAgentMcpRequests: 0
    },
    evidence: [
      'scripts/verify-packaged-playwright-startup.mjs',
      'scripts/verify-packaged-playwright-startup.test.mjs'
    ]
  },
  {
    id: 'packaged_application_offline_e2e',
    status: 'pending',
    scope:
      'Launch the unpacked production app offline and execute the local managed-browser fixture.',
    reasonCode: 'no_production_external_managed_mcp_driver',
    reason:
      'Process startup is observed, but the production app exposes no CLI, socket, environment hook, or general MCP endpoint that can drive Agent managed Playwright. Remote debugging, unknown MCP, user-profile reuse, and a fixture-only production backdoor are forbidden.'
  }
])

export async function buildPlaywrightConformanceReport(options = {}) {
  const root = options.repositoryRoot ? resolve(options.repositoryRoot) : REPOSITORY_ROOT
  const [catalog, manifest] = await Promise.all([
    readJson(resolve(root, relativeToRoot(UPSTREAM_CATALOG_PATH))),
    readJson(resolve(root, relativeToRoot(POLICY_MANIFEST_PATH)))
  ])
  validateFrozenInputs(catalog, manifest)

  const policyByName = new Map(manifest.tools.map((tool) => [tool.rawName, tool]))
  const tools = catalog.tools.map((upstreamTool) => {
    const policy = policyByName.get(upstreamTool.rawName)
    const typedPlatformUnavailable = policy.constraints.includes('typed_platform_unavailable')
    return {
      rawName: upstreamTool.rawName,
      modelName: policy.modelName,
      capability: upstreamTool.capability,
      handlingMode: policy.handlingMode,
      exposed: policy.exposed,
      availability: policy.exposed
        ? typedPlatformUnavailable
          ? 'model_visible_typed_unavailable'
          : 'model_visible'
        : 'model_hidden',
      upstreamSchemaDigest: upstreamTool.schemaDigest,
      hostOverlayDigest: policy.hostOverlayDigest,
      hostInputSchemaDigest: policy.hostInputSchemaDigest,
      hostOverlay: policy.hostOverlay,
      reasonCode: policy.reasonCode,
      constraints: policy.constraints,
      behaviorContract: typedPlatformUnavailable
        ? 'typed_platform_unavailable'
        : behaviorContractFor(policy.handlingMode)
    }
  })

  const handlingModes = Object.fromEntries(
    [...HANDLING_MODES].map((mode) => [
      mode,
      tools.filter((tool) => tool.handlingMode === mode).length
    ])
  )

  const securityLimits = await readSecurityLimits(root)
  return {
    schemaVersion: 1,
    reportKind: 'managed_playwright_release_conformance',
    evidenceSemantics: {
      contract: 'Enforced by deterministic source validation; this is not a runtime test result.',
      evidence:
        'A bounded automated evidence source is named; an execution is observed only when its command and result are explicitly recorded.',
      pending: 'The stated release claim is not yet established by a complete automated gate.'
    },
    package: {
      name: catalog.packageName,
      version: catalog.packageVersion,
      playwrightVersion: catalog.playwrightVersion
    },
    identity: {
      managedMcpId: manifest.managedMcpId,
      capabilityId: manifest.capabilityId,
      manifestVersion: manifest.manifestVersion,
      upstreamCatalogDigest: catalog.catalogDigest,
      policyDigest: manifest.policyDigest
    },
    classification: {
      status: 'contract',
      upstreamToolCount: tools.length,
      policyToolCount: manifest.tools.length,
      exposedToolCount: tools.filter((tool) => tool.exposed).length,
      hiddenToolCount: tools.filter((tool) => !tool.exposed).length,
      handlingModes
    },
    tools,
    schemaParity: {
      status: 'contract',
      upstreamSchemas: 'digest_locked_without_mutation',
      hostSchemas: 'upstream_schema_plus_digest_locked_host_overlay',
      overlayOnlyFields: ['call_reason'],
      recursiveKeywords: SCHEMA_PARITY_KEYWORDS,
      unknownFields: 'rejected_by_additionalProperties_false',
      providerPayloadEvidence: {
        status: 'evidence',
        execution: 'not_recorded_in_source_control',
        providers: ['openai_compatible', 'anthropic_compatible'],
        evidence: [
          'crates/core-server/src/application/mcp/playwright_manifest.rs',
          'crates/core/src/tools/builtin_capability.rs'
        ]
      }
    },
    behaviorDifferences: behaviorDifferences(tools),
    ownershipDifferences: {
      browser_close: {
        externalOwnedBrowser:
          'closes_the_owned_browser_and_the_next_call_may_lazily_start_a_new_browser',
        externalSharedContext: 'disposes_the_automation_context_wrapper_without_closing_pages',
        builtinManagedElectron:
          'retires_the_current_automation_generation_and_overlay_state_while_preserving_user_owned_visible_surfaces',
        singleTabCloseTool: 'browser_tabs_close'
      }
    },
    platformCapabilityDifferences: {
      browser_pdf_save: {
        platform: 'darwin-arm64',
        runtime: 'electron39_managed_guest',
        status: 'typed_unavailable',
        modelVisibility: 'visible',
        fixedUpstreamContract: 'page_pdf_via_page_print_to_pdf_return_as_stream',
        builtinEvidence:
          'managed_guest_debugger_rejects_page_print_to_pdf_before_cdp_dispatch_and_web_contents_print_to_pdf_does_not_settle',
        toolResult: {
          isError: true,
          status: 'unavailable',
          code: 'browser.pdf_unavailable',
          dispatchCertainty: 'response_received'
        },
        fallback: 'none',
        artifactPublished: false,
        evidence: [
          'src/main/core/browserTargetBroker.test.ts',
          'src/main/core/managedPlaywrightBridge.electron.test.ts',
          'src/main/browser/fixtures/managedPlaywrightBridge.electron.ts'
        ]
      }
    },
    knownBehaviorGaps: {
      popupOpenerSemantics: {
        status: 'not_native_parity',
        builtinBehavior: 'host_owned_independent_managed_webview_surface',
        targetAdmission: 'current_surface_group_only',
        nativeWindowProxy: false,
        openerReference: false,
        openerPostMessage: false,
        popupCloseLinkage: false,
        evidence: [
          'src/main/core/browserSurfaceManager.test.ts',
          'src/main/core/browserTargetBroker.test.ts'
        ]
      },
      workerTargets: {
        serviceWorker: { status: 'not_admitted', electronE2e: false },
        sharedWorker: { status: 'not_admitted', electronE2e: false },
        boundary: 'managed_page_frame_and_dedicated_worker_targets_only',
        evidence: ['src/main/core/browserTargetBroker.test.ts']
      }
    },
    sensitiveResourceScope: {
      status: 'evidence',
      execution: 'observed_on_2026-08-20',
      principle:
        'Main owns the active managed Surface or managed-profile authority. Approval binds exact arguments and a one-shot opaque Host binding; page-scoped tools additionally bind surface generation, navigation epoch, and the Host-observed origin.',
      observedEvidence: {
        platform: 'darwin-arm64',
        command:
          'pnpm exec vitest run --project unit src/main/core/managedPlaywrightBridge.electron.test.ts',
        result: 'passed',
        fixture: 'repository_local_loopback_only',
        externalWebsites: false,
        realUserBrowser: false
      },
      tools: [
        {
          tool: 'browser_evaluate',
          scope: 'managed_surface',
          available: [
            'page_context',
            'snapshot_ref_in_managed_child_frame',
            'frame_locator_in_managed_surface',
            'unique_selector_in_managed_surface'
          ],
          exactEffectEvidence:
            'fixed_official_0_0_79_mutates_and_reads_a_localhost_oopif_element_inside_the_selected_managed_guest',
          excluded: ['other_surface', 'main_renderer', 'unmanaged_guest']
        },
        {
          tool: 'browser_drop',
          scope: 'managed_surface',
          available: [
            'pure_mime_data_without_file_approval',
            'one_call_workspace_path_preflight_to_opaque_file_authority',
            'all_approved_paths_use_exact_frame_host_adapter',
            'oopif_dragenter_dragover_drop_order_and_effect',
            'large_file_host_adapter_without_base64'
          ],
          exactEffectEvidence:
            'host_adapter_preserves_the_fixed_target_grammar_and_drop_event_contract_while_the_localhost_oopif_observes_the_brokered_fixture_basename_bytes_and_event_order',
          fileBoundary: 'one_time_filebroker_lease_only'
        },
        {
          tool: 'browser_file_upload',
          scope: 'managed_surface',
          available: [
            'one_call_workspace_path_preflight_to_pending_managed_guest_file_chooser',
            'omitted_paths_preserves_official_cancel_chooser_semantics'
          ],
          exactEffectEvidence:
            'fixed_official_0_0_79_file_chooser_change_event_observes_the_brokered_fixture_basename_and_bytes',
          fileBoundary: 'one_time_filebroker_lease_only'
        },
        {
          tool: 'browser_network_request',
          scope: 'managed_surface',
          available: ['fixed_current_tab_request_index', 'headers_or_body_live_result'],
          exactEffectEvidence:
            'fixed_official_0_0_79_returns_the_exact_repository_fixture_ping_response_body_selected_from_the_safe_request_ledger',
          durableProjection: 'content_omitted',
          generationChange: 'resolved_by_the_rebuilt_fixed_current_tab_ledger'
        },
        {
          tool: 'browser_cookie_*',
          scope: 'managed_browser_profile',
          available: ['zero_tab_context_approval', 'no_active_origin_requirement'],
          durableProjection: 'content_omitted'
        },
        {
          tool: 'browser_storage_state/browser_set_storage_state',
          scope: 'managed_browser_profile',
          available: ['zero_tab_context_approval', 'background_target_intent_when_required'],
          fileBoundary: 'artifactbroker_export_and_one_time_filebroker_import'
        }
      ]
    },
    iframeParity: {
      status: 'evidence',
      execution: 'observed_on_2026-08-20',
      observedEvidence: {
        date: '2026-08-20',
        platform: 'darwin-arm64',
        result: 'passed',
        scope: 'real_electron_guest_oopif_input_delivery'
      },
      expectedParity: [
        'final_dom',
        'text_value',
        'focused_frame',
        'keyboard_event_order',
        'tool_result'
      ],
      browserEvaluateIsNotAcceptedAsTypeParity: true,
      managedGuestAdaptations: [
        {
          difference: 'Target.setAutoAttach.waitForDebuggerOnStart_is_forced_false',
          reason: 'Electron otherwise exposes an empty child frame tree and deadlocks OOPIF input.',
          isolation:
            'Page.createIsolatedWorld is routed only through the exact admitted guest root session; no global CDP endpoint or unrelated Target is exposed.'
        },
        {
          difference: 'blank_iframe_editor_candidates_use_one_time_host_tokens',
          reason:
            'An empty accessibility subtree has no upstream aria-ref; a positional nth selector could drift to a different editor after frame reordering.',
          isolation:
            'The token retains an exact ElementHandle in Main and is bound to run, activation, surface generation, and a 60-second TTL; it is consumed once and never persisted as a CDP or DOM identity.'
        }
      ],
      knownHarnessLimitations: [
        {
          scope: 'hidden_cargo_grandchild_fixture_only',
          limitation: 'slowly_true_widget_event_is_not_observed',
          productImpact: 'none_on_the_direct_visible_electron_product_parity_path'
        }
      ],
      safeErrorCodes: [
        'frame_not_found',
        'stale_frame_ref',
        'frame_not_editable',
        'frame_input_delivery_failed',
        'frame_detached',
        'target_closed'
      ],
      evidence: [
        'src/main/browser/fixtures/browserTargetBroker.electron.ts',
        'src/main/core/browserTargetBroker.electron.test.ts',
        'src/main/browser/fixtures/managedPlaywrightBridge.electron.ts'
      ]
    },
    security: {
      hostIsolation: {
        globalRemoteDebuggingPort: false,
        userBrowserConnection: false,
        unknownThirdPartyServerExecution: false,
        managedTargetsOnly: true
      },
      browserRunCodeUnsafe: {
        status: 'pending',
        modelVisible: false,
        conclusion: 'unavailable_without_a_real_low_privilege_process_sandbox',
        mainProcessExecutionForbidden: true
      },
      persistenceLeakBudget: {
        secretValues: 0,
        authorizationHeaders: 0,
        cookieValues: 0,
        storageValues: 0,
        fileContents: 0,
        absoluteHostPaths: 0,
        binaryOrBase64Payloads: 0
      },
      limits: securityLimits
    },
    stressEvidence: {
      status: 'evidence',
      execution: 'observed_on_2026-08-20',
      observedEvidence: {
        platform: 'darwin-arm64',
        command: 'pnpm test:playwright-round3-stress',
        result: 'passed',
        tests: 5
      },
      covered: [
        {
          scenario: 'managed_host_read_tool_dispatch',
          iterations: 500,
          executionBoundary: 'fixed_catalog_plus_in_memory_official_client_fixture',
          evidence:
            'src/main/mcp/managedPlaywrightRound3Stress.test.ts > settles 500 continuous Host calls and fails closed at the 32-reader admission boundary'
        },
        {
          scenario: 'concurrent_read_admission_attempts',
          iterations: 32,
          firstWaveAdmitted: 8,
          firstWaveRejectedBusy: 24,
          completedAfterBoundedRetry: 32,
          admissionWaves: 4,
          evidence:
            'src/main/mcp/managedPlaywrightRound3Stress.test.ts > settles 500 continuous Host calls and fails closed at the 32-reader admission boundary'
        },
        {
          scenario: 'tab_create_close',
          iterations: 100,
          evidence:
            'src/main/mcp/managedPlaywrightRound3Stress.test.ts > returns tabs, routes, context listeners, and connection temp directories to baseline for 100 cycles'
        },
        {
          scenario: 'route_unroute',
          iterations: 100,
          evidence:
            'src/main/mcp/managedPlaywrightRound3Stress.test.ts > returns tabs, routes, context listeners, and connection temp directories to baseline for 100 cycles'
        },
        {
          scenario: 'file_selection_and_consume',
          iterations: 100,
          evidence:
            'src/main/mcp/managedPlaywrightRound3Stress.test.ts > consumes 100 local-file authorities and removes every private copy with an injected clock'
        },
        {
          scenario: 'protected_storage_export_and_revoke',
          iterations: 100,
          evidence:
            'src/main/mcp/managedPlaywrightRound3Stress.test.ts > exports and revokes 100 protected storage states without retaining artifacts, sessions, timers, or secrets'
        },
        {
          scenario: 'target_crash_recovery',
          iterations: 25,
          executionBoundary: 'real_broker_and_transport_with_local_webcontents_fixture',
          evidence:
            'src/main/mcp/managedPlaywrightRound3Stress.test.ts > recovers 25 target crashes and 25 failed relay generations with all listeners and timers at baseline'
        },
        {
          scenario: 'relay_generation_recovery',
          iterations: 25,
          executionBoundary: 'real_bridge_with_typed_local_core_and_host_fixtures',
          evidence:
            'src/main/mcp/managedPlaywrightRound3Stress.test.ts > recovers 25 target crashes and 25 failed relay generations with all listeners and timers at baseline'
        },
        {
          scenario: 'activation_idle_reactivation',
          iterations: 100,
          evidence:
            'crates/core-server/src/application/mcp/managed_playwright_bridge.rs > idle_policy_reuses_one_timer_and_reactivates_for_one_hundred_cycles'
        },
        {
          scenario: 'approve_reject_race',
          iterations: 1,
          evidence:
            'crates/core-server/src/application/mcp/builtin_capability_runtime.rs > approve_reject_race_has_exactly_one_terminal_authority'
        },
        {
          scenario: 'cdp_child_attachment_storm',
          iterations: securityLimits.cdp.maxChildSessions + 1,
          evidence:
            'src/main/core/browserTargetBroker.test.ts > closes on an OOPIF attachment storm before child session state can grow unbounded'
        },
        {
          scenario: 'mcp_notification_queue_bound',
          iterations: 1025,
          evidence: 'src/main/core/ipc.mcp.test.ts'
        }
      ],
      pending: [
        'real_electron_browser_tool_calls_500',
        'real_electron_tab_cycles_100',
        'upload_download_cycles_100',
        'real_renderer_and_server_process_crash_cycles_25_each',
        'rss_non_linear_growth_release_gate'
      ]
    },
    packagedApplication: {
      status: 'evidence',
      execution: 'observed_on_2026-08-20',
      observedEvidence: {
        platform: 'darwin-arm64',
        command: 'pnpm verify:playwright-packaged-startup',
        result: 'passed',
        scope: 'unpacked_macos_process_startup_only',
        sourceFreshness: 'not_established_gate_verifies_the_supplied_bundle_only',
        bundledCoreServerProcesses: 1,
        sandboxedRendererProcesses: 1,
        stableSamples: 3,
        temporaryAppData: true,
        proxiedOutboundAttempts: 0,
        localFixtureSelfCheck: 'passed'
      },
      isolation: {
        inheritedMyCopilotEnvironment: 'removed',
        userBrowserOrProfile: false,
        remoteDebugging: false,
        unknownMcp: false
      },
      offlineAssurance: 'proxy_and_resolver_containment_not_a_kernel_firewall',
      agentMcpLocalFixtureE2e: {
        status: 'pending',
        reasonCode: 'no_production_external_managed_mcp_driver',
        localFixtureAgentMcpRequests: 0,
        nearestExecutableEvidence:
          'pnpm exec vitest run --project unit src/main/core/managedPlaywrightBridge.electron.test.ts',
        requiredToClose:
          'Use the public packaged UI with an offline provider fixture and normal approval UI; do not add a privileged test channel.'
      }
    },
    platforms: [
      {
        platform: 'darwin',
        architecture: 'arm64',
        status: 'evidence',
        execution: 'observed_on_2026-08-20',
        scope: ['catalog', 'unit', 'local_electron_fixture', 'unpacked_process_startup'],
        observedTests: {
          catalogAndExternalReference: 8,
          directElectronOopif: 1,
          managedPlaywrightElectron: 1,
          round3DeterministicStress: 5,
          packagedStartupGate: 1
        },
        packagedOfflineStartup: 'observed_process_only',
        packagedOfflineE2e: 'pending'
      },
      {
        platform: 'windows',
        architecture: 'unverified',
        status: 'pending',
        reason: 'platform_environment_unavailable'
      },
      {
        platform: 'linux',
        architecture: 'unverified',
        status: 'pending',
        reason: 'platform_environment_unavailable'
      }
    ],
    releaseGates: RELEASE_GATES
  }
}

export async function serializePlaywrightConformanceReport(options) {
  const configuration = (await resolveConfig(PLAYWRIGHT_CONFORMANCE_REPORT_PATH)) ?? {}
  return await format(JSON.stringify(await buildPlaywrightConformanceReport(options)), {
    ...configuration,
    filepath: PLAYWRIGHT_CONFORMANCE_REPORT_PATH,
    parser: 'json'
  })
}

export async function writePlaywrightConformanceReport(options) {
  const reportPath = options?.reportPath ?? PLAYWRIGHT_CONFORMANCE_REPORT_PATH
  await writeFile(reportPath, await serializePlaywrightConformanceReport(options), 'utf8')
}

export async function checkPlaywrightConformanceReport(options) {
  const reportPath = options?.reportPath ?? PLAYWRIGHT_CONFORMANCE_REPORT_PATH
  const [expected, actual] = await Promise.all([
    serializePlaywrightConformanceReport(options),
    readFile(reportPath, 'utf8')
  ])
  if (actual !== expected) throw new Error('playwright_conformance_report_stale')
}

function behaviorContractFor(mode) {
  switch (mode) {
    case 'pass_through':
      return 'fixed_official_server_passthrough'
    case 'host_adapted':
      return 'typed_host_lifecycle_adapter'
    case 'approval_required':
      return 'original_tool_internal_approval_then_fixed_official_server'
    case 'artifact_managed':
      return 'host_owned_artifact_reference'
    case 'sandboxed':
      return 'unavailable_until_real_process_sandbox'
    case 'unsupported':
      return 'typed_model_hidden_unavailable'
    default:
      throw new Error(`unknown_handling_mode:${mode}`)
  }
}

function behaviorDifferences(tools) {
  return [...HANDLING_MODES].map((handlingMode) => ({
    handlingMode,
    toolCount: tools.filter((tool) => tool.handlingMode === handlingMode).length,
    contract: behaviorContractFor(handlingMode),
    expectedDifference:
      handlingMode === 'pass_through'
        ? 'none_beyond_host_overlay_and_managed_target_binding'
        : handlingMode === 'approval_required'
          ? 'approval_is_inside_the_original_tool_lifecycle'
          : handlingMode === 'artifact_managed'
            ? 'binary_or_file_output_is_replaced_by_a_safe_artifact_reference'
            : handlingMode === 'host_adapted'
              ? 'host_lifecycle_or_surface_boundary_is_authoritative'
              : 'tool_is_not_model_visible'
  }))
}

async function readSecurityLimits(root) {
  return {
    managedMcpHost: await constantsFromSource(root, 'src/main/mcp/ManagedPlaywrightMcpHost.ts', {
      maxActiveCalls: 'MAX_ACTIVE_CALLS',
      maxArgumentBytes: 'MAX_ARGUMENT_BYTES',
      maxArgumentDepth: 'MAX_ARGUMENT_DEPTH',
      maxArgumentNodes: 'MAX_ARGUMENT_NODES',
      maxResultBytes: 'MAX_RESULT_BYTES',
      maxTextBytes: 'MAX_TEXT_BYTES',
      maxStructuredContentBytes: 'MAX_STRUCTURED_CONTENT_BYTES'
    }),
    cdp: await constantsFromSource(root, 'src/main/browser/ElectronGuestCdpTransport.ts', {
      maxInsertTextBytes: 'MAX_INSERT_TEXT_BYTES',
      maxPayloadBytes: 'MAX_CDP_PAYLOAD_BYTES',
      maxPayloadDepth: 'MAX_CDP_PAYLOAD_DEPTH',
      maxPayloadNodes: 'MAX_CDP_PAYLOAD_NODES',
      maxEventQueue: 'MAX_CDP_EVENT_QUEUE',
      maxEventsPerSecond: 'MAX_CDP_EVENTS_PER_SECOND',
      maxChildSessions: 'MAX_CHILD_SESSIONS'
    }),
    artifacts: await constantsFromSource(root, 'src/main/browser/BrowserArtifactBroker.ts', {
      maxArtifactBytes: 'DEFAULT_MAX_ARTIFACT_BYTES',
      maxArtifacts: 'DEFAULT_MAX_ARTIFACTS',
      maxArtifactsPerRun: 'DEFAULT_MAX_ARTIFACTS_PER_RUN',
      maxRunBytes: 'DEFAULT_MAX_RUN_BYTES',
      maxTotalBytes: 'DEFAULT_MAX_TOTAL_BYTES',
      ttlMs: 'DEFAULT_TTL_MS'
    }),
    files: await constantsFromSource(root, 'src/main/browser/BrowserFileBroker.ts', {
      maxFileBytes: 'DEFAULT_MAX_FILE_BYTES',
      maxRunBytes: 'DEFAULT_MAX_RUN_BYTES',
      maxFilesPerSelection: 'DEFAULT_MAX_FILES',
      maxHandles: 'DEFAULT_MAX_HANDLES',
      ttlMs: 'DEFAULT_TTL_MS'
    }),
    targets: await constantsFromSource(root, 'src/main/browser/BrowserTargetBroker.ts', {
      maxRegisteredGuests: 'MAX_REGISTERED_GUESTS'
    })
  }
}

async function constantsFromSource(root, source, fields) {
  const content = await readFile(resolve(root, source), 'utf8')
  return Object.fromEntries(
    Object.entries(fields).map(([field, constant]) => [field, numericConstant(content, constant)])
  )
}

function numericConstant(content, name) {
  const match = content.match(
    new RegExp(`(?:static\\s+readonly\\s+|const\\s+)${name}\\s*=\\s*([^;\\n]+)`)
  )
  if (!match) throw new Error(`missing_release_limit:${name}`)
  const factors = match[1]
    .trim()
    .split('*')
    .map((factor) => Number(factor.trim().replaceAll('_', '')))
  if (factors.length === 0 || factors.some((factor) => !Number.isSafeInteger(factor))) {
    throw new Error(`invalid_release_limit:${name}`)
  }
  return factors.reduce((product, factor) => product * factor, 1)
}

function validateFrozenInputs(catalog, manifest) {
  if (
    catalog.schemaVersion !== 1 ||
    catalog.packageName !== '@playwright/mcp' ||
    catalog.packageVersion !== '0.0.79' ||
    !Array.isArray(catalog.tools) ||
    catalog.tools.length !== 69
  ) {
    throw new Error('invalid_upstream_catalog')
  }
  const catalogMaterial = structuredClone(catalog)
  delete catalogMaterial.catalogDigest
  if (
    !SHA256_DIGEST.test(catalog.catalogDigest) ||
    digestJson(catalogMaterial) !== catalog.catalogDigest
  ) {
    throw new Error('invalid_upstream_catalog_digest')
  }
  for (const tool of catalog.tools) {
    if (
      !tool ||
      !TOOL_NAME.test(tool.rawName) ||
      !SHA256_DIGEST.test(tool.schemaDigest) ||
      digestJson(tool.inputSchema) !== tool.schemaDigest
    ) {
      throw new Error('invalid_upstream_tool_digest')
    }
  }
  if (
    manifest.schemaVersion !== 2 ||
    manifest.packageName !== catalog.packageName ||
    manifest.packageVersion !== catalog.packageVersion ||
    manifest.upstreamCatalogDigest !== catalog.catalogDigest ||
    !Array.isArray(manifest.tools) ||
    manifest.tools.length !== catalog.tools.length
  ) {
    throw new Error('invalid_policy_manifest')
  }
  const policyMaterial = structuredClone(manifest)
  delete policyMaterial.policyDigest
  if (
    !SHA256_DIGEST.test(manifest.policyDigest) ||
    digestJson(policyMaterial) !== manifest.policyDigest
  ) {
    throw new Error('invalid_policy_digest')
  }

  const upstreamByName = new Map(catalog.tools.map((tool) => [tool.rawName, tool]))
  if (upstreamByName.size !== catalog.tools.length) throw new Error('duplicate_upstream_tool')
  const policyNames = new Set()
  for (const policy of manifest.tools) {
    const upstream = upstreamByName.get(policy.rawName)
    if (
      !upstream ||
      policyNames.has(policy.rawName) ||
      !TOOL_NAME.test(policy.rawName) ||
      !TOOL_NAME.test(policy.modelName) ||
      !HANDLING_MODES.has(policy.handlingMode) ||
      (policy.exposed === true && !EXPOSED_HANDLING_MODES.has(policy.handlingMode)) ||
      typeof policy.exposed !== 'boolean' ||
      policy.upstreamSchemaDigest !== upstream.schemaDigest ||
      !SHA256_DIGEST.test(policy.hostOverlayDigest) ||
      !SHA256_DIGEST.test(policy.hostInputSchemaDigest) ||
      digestJson(policy.hostOverlay) !== policy.hostOverlayDigest ||
      digestJson(applyHostOverlay(upstream.inputSchema, policy.hostOverlay)) !==
        policy.hostInputSchemaDigest ||
      Object.hasOwn(policy.hostOverlay ?? {}, 'addApprovalOrigin')
    ) {
      throw new Error('policy_catalog_drift')
    }
    policyNames.add(policy.rawName)
  }
  if (policyNames.size !== upstreamByName.size) throw new Error('unclassified_upstream_tool')
  const exposed = manifest.tools.filter((tool) => tool.exposed).length
  if (manifest.exposedToolCount !== exposed) throw new Error('exposed_tool_count_drift')
}

function applyHostOverlay(upstreamSchema, overlay) {
  if (
    !isRecord(upstreamSchema) ||
    upstreamSchema.type !== 'object' ||
    upstreamSchema.additionalProperties !== false ||
    !isRecord(upstreamSchema.properties) ||
    !isRecord(overlay) ||
    overlay.addCallReason !== true ||
    Object.keys(overlay).some(
      (key) => !['addCallReason', 'removeProperties', 'propertyOverrides'].includes(key)
    )
  ) {
    throw new Error('invalid_host_overlay')
  }
  const schema = structuredClone(upstreamSchema)
  const properties = schema.properties
  if (Object.hasOwn(properties, 'call_reason')) throw new Error('invalid_host_overlay')
  const required = Array.isArray(schema.required) ? schema.required : []
  const removed = new Set()
  for (const name of overlay.removeProperties ?? []) {
    if (
      typeof name !== 'string' ||
      removed.has(name) ||
      required.includes(name) ||
      !Object.hasOwn(properties, name)
    ) {
      throw new Error('invalid_host_overlay')
    }
    removed.add(name)
    delete properties[name]
  }
  if (overlay.propertyOverrides !== undefined && !isRecord(overlay.propertyOverrides)) {
    throw new Error('invalid_host_overlay')
  }
  for (const [name, override] of Object.entries(overlay.propertyOverrides ?? {})) {
    if (!isRecord(properties[name]) || !isRecord(override)) throw new Error('invalid_host_overlay')
    const overrideKeys = Object.keys(override)
    if (
      overrideKeys.length === 0 ||
      overrideKeys.some((key) => !['description', 'enum', 'default'].includes(key))
    ) {
      throw new Error('invalid_host_overlay')
    }
    properties[name] = deepMerge(properties[name], override)
  }
  properties.call_reason = structuredClone(HOST_CALL_REASON_SCHEMA)
  schema.required = [...new Set([...required, 'call_reason'])]
  return schema
}

function deepMerge(base, overlay) {
  const merged = structuredClone(base)
  for (const [key, value] of Object.entries(overlay)) {
    merged[key] =
      isRecord(merged[key]) && isRecord(value)
        ? deepMerge(merged[key], value)
        : structuredClone(value)
  }
  return merged
}

function digestJson(value) {
  return `sha256:${createHash('sha256')
    .update(JSON.stringify(canonicalJson(value)))
    .digest('hex')}`
}

function canonicalJson(value) {
  if (Array.isArray(value)) return value.map(canonicalJson)
  if (isRecord(value)) {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((key) => [key, canonicalJson(value[key])])
    )
  }
  return value
}

function isRecord(value) {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

async function readJson(path) {
  return JSON.parse(await readFile(path, 'utf8'))
}

function relativeToRoot(path) {
  return path.slice(REPOSITORY_ROOT.length + 1)
}

const invokedPath = process.argv[1] ? resolve(process.argv[1]) : ''
if (invokedPath === fileURLToPath(import.meta.url)) {
  const command = process.argv[2] ?? '--check'
  if (command === '--write') await writePlaywrightConformanceReport()
  else if (command === '--check') await checkPlaywrightConformanceReport()
  else throw new Error(`unknown_command:${command}`)
}
