import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import {
  MCP_BUILTIN_CAPABILITY_LIST_METHOD,
  MCP_BUILTIN_CAPABILITY_SET_ALLOWED_METHOD,
  MCP_CATALOG_REFRESH_METHOD,
  MCP_CATALOG_TOOLS_METHOD,
  MCP_CHANGED_NOTIFICATION_METHOD,
  MCP_MANAGEMENT_ERROR_CODE,
  MCP_MANAGEMENT_SCHEMA_VERSION,
  MCP_SERVER_ADD_METHOD,
  MCP_SERVER_AUTHORIZE_LAUNCH_COMMIT_METHOD,
  MCP_SERVER_AUTHORIZE_LAUNCH_PREPARE_METHOD,
  MCP_SERVER_DELETE_METHOD,
  MCP_SERVER_DISABLE_METHOD,
  MCP_SERVER_ENABLE_METHOD,
  MCP_SERVER_GET_METHOD,
  MCP_SERVER_LIST_METHOD,
  MCP_SERVER_RESTART_METHOD,
  MCP_SERVER_START_METHOD,
  MCP_SERVER_STATUS_METHOD,
  MCP_SERVER_STOP_METHOD,
  MCP_SERVER_UPDATE_METHOD,
  parseMcpBuiltinCapabilityListInput,
  parseMcpBuiltinCapabilityListOutput,
  parseMcpBuiltinCapabilityMutationOutput,
  parseMcpBuiltinCapabilitySetAllowedInput,
  parseMcpCatalogToolsPageInput,
  parseMcpCatalogToolsPageOutput,
  parseMcpChangedNotification,
  parseMcpLaunchAuthorizationCommitInput,
  parseMcpLaunchAuthorizationPreview,
  parseMcpLaunchAuthorizationResult,
  parseMcpManagementErrorData,
  parseMcpServerCreateInput,
  parseMcpServerDetailsOutput,
  parseMcpServerIdInput,
  parseMcpServerListInput,
  parseMcpServerListOutput,
  parseMcpServerMutationInput,
  parseMcpServerUpdateInput
} from '..'

const golden = JSON.parse(
  readFileSync(
    resolve(process.cwd(), 'packages/protocol/fixtures/mcp-management-contract-v1.json'),
    'utf8'
  )
)
const builtinGolden = JSON.parse(
  readFileSync(
    resolve(process.cwd(), 'packages/protocol/fixtures/mcp-builtin-capability-contract-v1.json'),
    'utf8'
  )
)

const serverId = 'ce18d23c-e74f-4e89-8695-ce1e7c60ec92'
const configEpoch = '41818332-0842-4d2e-808f-175b70eb4628'
const authorizationId = '8e9a3118-88f4-4a53-9dba-82f89232d07e'
const sourceEpoch = 'd8346f56-c0f2-4e9d-b394-74dfd9e959e1'
const configDigest = 'a'.repeat(64)

const listItem = {
  schemaVersion: 1,
  serverId,
  displayName: 'Owned fixture',
  scope: 'user',
  source: 'userManual',
  transport: 'stdio',
  enabled: false,
  trust: 'untrusted',
  approvalMode: 'prompt',
  launchAuthorizationState: 'required',
  state: 'disabled',
  registryRevision: 7,
  configEpoch,
  configDigest,
  catalogGeneration: 0,
  catalogCompleteness: 'failed',
  toolCount: 0,
  activeCallCount: 0,
  updatedAtMs: 1_753_843_200_000
} as const

const details = {
  ...listItem,
  executable: '/owned/fixture',
  arguments: ['--mode', '', 'value with spaces;$(not-a-shell)'],
  cwd: '/owned',
  capabilities: {
    tools: true,
    resources: false,
    prompts: false,
    logging: false,
    completion: false
  },
  createdAtMs: 1_753_843_100_000
} as const

describe('MCP management cross-language contract', () => {
  it('keeps built-in capability policy separate from external Server lifecycle', () => {
    expect(builtinGolden.methods).toEqual({
      list: MCP_BUILTIN_CAPABILITY_LIST_METHOD,
      setAllowed: MCP_BUILTIN_CAPABILITY_SET_ALLOWED_METHOD
    })
    expect(parseMcpBuiltinCapabilityListInput(builtinGolden.listInput)).toEqual(
      builtinGolden.listInput
    )
    expect(parseMcpBuiltinCapabilitySetAllowedInput(builtinGolden.setAllowedInput)).toEqual(
      builtinGolden.setAllowedInput
    )
    expect(parseMcpBuiltinCapabilityListOutput(builtinGolden.listOutput)).toEqual(
      builtinGolden.listOutput
    )
    expect(parseMcpBuiltinCapabilityMutationOutput(builtinGolden.mutationOutput)).toEqual(
      builtinGolden.mutationOutput
    )

    expect(() =>
      parseMcpBuiltinCapabilitySetAllowedInput({
        ...builtinGolden.setAllowedInput,
        enabled: true
      })
    ).toThrow(/unexpected field/)
    expect(() =>
      parseMcpBuiltinCapabilitySetAllowedInput({
        ...builtinGolden.setAllowedInput,
        capabilityId: 'external_server'
      })
    ).toThrow(/unexpected value/)
    for (const forbidden of ['state', 'serverId', 'manifest', 'tools', 'executable']) {
      expect(() =>
        parseMcpBuiltinCapabilityListOutput({
          ...builtinGolden.listOutput,
          capabilities: [
            { ...builtinGolden.listOutput.capabilities[0], [forbidden]: 'must-not-cross' }
          ]
        })
      ).toThrow(/unexpected field/)
    }
  })

  it('keeps stable methods, schema and error namespace aligned with Rust', () => {
    expect(MCP_MANAGEMENT_SCHEMA_VERSION).toBe(golden.schemaVersion)
    expect(MCP_MANAGEMENT_ERROR_CODE).toBe(golden.errorCode)
    expect(golden.methods).toEqual({
      list: MCP_SERVER_LIST_METHOD,
      get: MCP_SERVER_GET_METHOD,
      add: MCP_SERVER_ADD_METHOD,
      update: MCP_SERVER_UPDATE_METHOD,
      delete: MCP_SERVER_DELETE_METHOD,
      prepareLaunchAuthorization: MCP_SERVER_AUTHORIZE_LAUNCH_PREPARE_METHOD,
      commitLaunchAuthorization: MCP_SERVER_AUTHORIZE_LAUNCH_COMMIT_METHOD,
      enable: MCP_SERVER_ENABLE_METHOD,
      disable: MCP_SERVER_DISABLE_METHOD,
      start: MCP_SERVER_START_METHOD,
      stop: MCP_SERVER_STOP_METHOD,
      restart: MCP_SERVER_RESTART_METHOD,
      status: MCP_SERVER_STATUS_METHOD,
      listTools: MCP_CATALOG_TOOLS_METHOD,
      refreshCatalog: MCP_CATALOG_REFRESH_METHOD,
      changed: MCP_CHANGED_NOTIFICATION_METHOD
    })
  })

  it('round-trips every shared Rust/TypeScript golden DTO through strict parsers', () => {
    expect(parseMcpServerListInput(golden.listInput)).toEqual(golden.listInput)
    expect(parseMcpServerIdInput(golden.idInput)).toEqual(golden.idInput)
    expect(parseMcpServerMutationInput(golden.mutationInput)).toEqual(golden.mutationInput)
    expect(parseMcpServerCreateInput(golden.createInput)).toEqual(golden.createInput)
    expect(parseMcpServerUpdateInput(golden.updateInput)).toEqual(golden.updateInput)
    expect(parseMcpServerListOutput(golden.listOutput)).toEqual(golden.listOutput)
    expect(parseMcpServerDetailsOutput(golden.detailsOutput)).toEqual(golden.detailsOutput)
    expect(() =>
      parseMcpServerDetailsOutput({
        ...golden.detailsOutput,
        server: {
          ...golden.detailsOutput.server,
          protocol: {
            ...golden.detailsOutput.server.protocol,
            lifecycle: 'serverExtension'
          }
        }
      })
    ).toThrow(/unexpected value/)
    expect(parseMcpLaunchAuthorizationPreview(golden.authorization.preview)).toEqual(
      golden.authorization.preview
    )
    expect(parseMcpLaunchAuthorizationCommitInput(golden.authorization.commitInput)).toEqual(
      golden.authorization.commitInput
    )
    expect(parseMcpLaunchAuthorizationResult(golden.authorization.result)).toEqual(
      golden.authorization.result
    )
    expect(parseMcpCatalogToolsPageInput(golden.catalog.input)).toEqual(golden.catalog.input)
    expect(() => parseMcpCatalogToolsPageInput({ ...golden.catalog.input, cursor: null })).toThrow()
    expect(parseMcpCatalogToolsPageOutput(golden.catalog.output)).toEqual(golden.catalog.output)
    expect(parseMcpManagementErrorData(golden.error)).toEqual(golden.error)
    expect(parseMcpChangedNotification(golden.changed)).toEqual(golden.changed)
    expect(parseMcpChangedNotification(golden.resyncChanged)).toEqual(golden.resyncChanged)
  })

  it('preserves ordered argv including empty and shell-looking values without shell parsing', () => {
    const value = {
      schemaVersion: 1,
      displayName: 'Owned fixture',
      transport: 'stdio',
      executable: '/owned/fixture',
      arguments: ['--mode', '', 'value with spaces;$(not-a-shell)'],
      cwd: '/owned',
      approvalMode: 'prompt'
    }
    expect(parseMcpServerCreateInput(value)).toEqual(value)
  })

  it.each(['prompt', 'auto', 'deny'] as const)(
    'round-trips the supported %s approval mode',
    (approvalMode) => {
      const value = {
        schemaVersion: 1,
        displayName: 'Owned fixture',
        transport: 'stdio',
        executable: '/owned/fixture',
        arguments: [],
        cwd: '/owned',
        approvalMode
      } as const
      expect(parseMcpServerCreateInput(value)).toEqual(value)
    }
  )

  it('rejects unknown approval modes', () => {
    expect(() =>
      parseMcpServerCreateInput({
        schemaVersion: 1,
        displayName: 'Owned fixture',
        transport: 'stdio',
        executable: '/owned/fixture',
        arguments: [],
        cwd: '/owned',
        approvalMode: 'always'
      })
    ).toThrow(/unexpected value/)
  })

  it('requires approvalMode on create and update requests', () => {
    const createWithoutApprovalMode: Record<string, unknown> = { ...golden.createInput }
    const updateWithoutApprovalMode: Record<string, unknown> = { ...golden.updateInput }
    delete createWithoutApprovalMode.approvalMode
    delete updateWithoutApprovalMode.approvalMode

    expect(() => parseMcpServerCreateInput(createWithoutApprovalMode)).toThrow(/approvalMode/)
    expect(() => parseMcpServerUpdateInput(updateWithoutApprovalMode)).toThrow(/approvalMode/)
  })

  it.each(['environment', 'secretRef', 'headers', 'bearer', 'url', 'serverId', 'trust', 'command'])(
    'rejects forbidden create field %s',
    (field) => {
      expect(() =>
        parseMcpServerCreateInput({
          schemaVersion: 1,
          displayName: 'Owned fixture',
          transport: 'stdio',
          executable: '/owned/fixture',
          arguments: [],
          cwd: '/owned',
          approvalMode: 'prompt',
          [field]: 'fixed-canary-must-not-cross'
        })
      ).toThrow(/unexpected field/)
    }
  )

  it('validates list and flattened details projections without exposing launch secrets', () => {
    expect(
      parseMcpServerListOutput({
        schemaVersion: 1,
        registryRevision: 7,
        servers: [listItem]
      })
    ).toEqual({
      schemaVersion: 1,
      registryRevision: 7,
      servers: [listItem]
    })
    expect(() =>
      parseMcpServerListOutput({
        schemaVersion: 1,
        registryRevision: 6,
        servers: [listItem]
      })
    ).toThrow(/must not exceed response revision/)
    expect(
      parseMcpServerDetailsOutput({
        schemaVersion: 1,
        registryRevision: 7,
        server: details
      })
    ).toEqual({
      schemaVersion: 1,
      registryRevision: 7,
      server: details
    })

    for (const forbidden of ['environment', 'stderr', 'secretRef', 'authorization', 'rawResult']) {
      expect(() =>
        parseMcpServerDetailsOutput({
          schemaVersion: 1,
          registryRevision: 7,
          server: { ...details, [forbidden]: 'fixed-canary-must-not-cross' }
        })
      ).toThrow(/unexpected field/)
    }
  })

  it('validates frozen launch preview and commit result projections', () => {
    const preview = {
      schemaVersion: 1,
      authorizationId,
      expiresAtMs: 1_753_843_260_000,
      serverId,
      displayName: 'Owned fixture',
      executable: '/owned/fixture',
      arguments: details.arguments,
      cwd: '/owned',
      launchSpecDigest: 'b'.repeat(64),
      precondition: {
        expectedRegistryRevision: 7,
        expectedConfigEpoch: configEpoch,
        expectedConfigDigest: configDigest
      }
    }
    expect(parseMcpLaunchAuthorizationPreview(preview)).toEqual(preview)
    expect(() =>
      parseMcpServerMutationInput({
        ...golden.mutationInput,
        precondition: {
          ...golden.mutationInput.precondition,
          expectedRegistryRevision: 0
        }
      })
    ).toThrow(/greater than or equal to 1/)
    expect(
      parseMcpLaunchAuthorizationResult({
        schemaVersion: 1,
        authorized: true,
        server: { ...details, trust: 'userApproved', launchAuthorizationState: 'authorized' }
      })
    ).toMatchObject({ authorized: true })
  })

  it('validates safe Catalog pages and rejects schemas and metadata', () => {
    const tool = {
      serverId,
      rawName: 'echo_text',
      modelName: 'mcp__owned_fixture__echo_text',
      routable: true,
      disabled: false,
      schemaDigestPrefix: '0123456789ab',
      description: 'Echo text.',
      descriptionTruncated: false,
      diagnosticCodes: [],
      catalogGeneration: 2,
      catalogCompleteness: 'complete'
    }
    const page = {
      schemaVersion: 1,
      serverId,
      catalogGeneration: 2,
      catalogCompleteness: 'complete',
      tools: [tool]
    }
    expect(parseMcpCatalogToolsPageOutput(page)).toEqual(page)

    for (const forbidden of ['inputSchema', 'outputSchema', '_meta', 'annotations', 'rawCursor']) {
      expect(() =>
        parseMcpCatalogToolsPageOutput({
          ...page,
          tools: [{ ...tool, [forbidden]: { canary: 'fixed-canary-must-not-cross' } }]
        })
      ).toThrow(/unexpected field/)
    }
  })

  it.each([
    ['C0 control', '\n'],
    ['bidi override', '\u202e'],
    ['bidi isolate', '\u2066'],
    ['line separator', '\u2028'],
    ['paragraph separator', '\u2029']
  ])('rejects %s in every untrusted display-text projection', (_label, disallowed) => {
    expect(() =>
      parseMcpServerCreateInput({
        schemaVersion: 1,
        displayName: `Owned${disallowed}fixture`,
        transport: 'stdio',
        executable: '/owned/fixture',
        arguments: [],
        cwd: '/owned',
        approvalMode: 'prompt'
      })
    ).toThrow(/disallowed control/)

    expect(() =>
      parseMcpServerDetailsOutput({
        schemaVersion: 1,
        registryRevision: 7,
        server: {
          ...details,
          protocol: {
            protocolVersion: `2026${disallowed}07-28`,
            lifecycle: 'discover'
          }
        }
      })
    ).toThrow(/disallowed control/)

    expect(() =>
      parseMcpCatalogToolsPageOutput({
        schemaVersion: 1,
        serverId,
        catalogGeneration: 2,
        catalogCompleteness: 'complete',
        tools: [
          {
            serverId,
            rawName: 'echo_text',
            modelName: 'mcp__owned_fixture__echo_text',
            routable: true,
            disabled: false,
            schemaDigestPrefix: '0123456789ab',
            description: `Echo${disallowed}text.`,
            descriptionTruncated: false,
            diagnosticCodes: [],
            catalogGeneration: 2,
            catalogCompleteness: 'complete'
          }
        ]
      })
    ).toThrow(/disallowed control/)
  })

  it('accepts replacement-marked Catalog text and rejects raw/model-name controls', () => {
    const safeProjectedTool = {
      serverId,
      rawName: 'echo\ufffdtext',
      modelName: 'mcp__owned\ufffdfixture__echo',
      routable: true,
      disabled: false,
      schemaDigestPrefix: '0123456789ab',
      description: 'Projected\ufffddescription',
      descriptionTruncated: true,
      diagnosticCodes: [],
      catalogGeneration: 2,
      catalogCompleteness: 'complete'
    }
    const page = {
      schemaVersion: 1,
      serverId,
      catalogGeneration: 2,
      catalogCompleteness: 'complete',
      tools: [safeProjectedTool]
    }
    expect(parseMcpCatalogToolsPageOutput(page)).toEqual(page)
    expect(() =>
      parseMcpCatalogToolsPageOutput({
        ...page,
        tools: [{ ...safeProjectedTool, rawName: 'echo\u2066text' }]
      })
    ).toThrow(/disallowed control/)
    expect(() =>
      parseMcpCatalogToolsPageOutput({
        ...page,
        tools: [{ ...safeProjectedTool, modelName: 'mcp__owned\u202efixture' }]
      })
    ).toThrow(/disallowed control/)
  })

  it('accepts only bounded structured management errors and notifications', () => {
    const error = {
      schemaVersion: 1,
      type: 'mcpManagement',
      operation: 'enable',
      code: 'authorizationRequired',
      recovery: 'requestLaunchAuthorization',
      message: 'Authorize the current launch configuration.',
      serverId,
      currentRegistryRevision: 7
    }
    expect(parseMcpManagementErrorData(error)).toEqual(error)
    expect(
      parseMcpChangedNotification({
        schemaVersion: 1,
        sourceEpoch,
        sequence: 9,
        registryRevision: 7,
        kind: 'stateChanged',
        serverId,
        state: 'ready'
      })
    ).toEqual({
      schemaVersion: 1,
      sourceEpoch,
      sequence: 9,
      registryRevision: 7,
      kind: 'stateChanged',
      serverId,
      state: 'ready'
    })

    expect(() =>
      parseMcpManagementErrorData({ ...error, stderr: 'fixed-canary-must-not-cross' })
    ).toThrow(/unexpected field/)
    expect(() =>
      parseMcpChangedNotification({
        schemaVersion: 1,
        sourceEpoch,
        sequence: 10,
        registryRevision: 7,
        kind: 'stateChanged',
        serverId,
        rawArguments: 'fixed-canary-must-not-cross'
      })
    ).toThrow(/unexpected field/)

    expect(
      parseMcpChangedNotification({
        schemaVersion: 1,
        sourceEpoch,
        sequence: 11,
        registryRevision: 7,
        kind: 'resyncRequired'
      })
    ).toEqual({
      schemaVersion: 1,
      sourceEpoch,
      sequence: 11,
      registryRevision: 7,
      kind: 'resyncRequired'
    })
    expect(() =>
      parseMcpChangedNotification({
        schemaVersion: 1,
        sourceEpoch,
        sequence: 12,
        registryRevision: 7,
        kind: 'resyncRequired',
        serverId
      })
    ).toThrow(/must not claim a Server/)
    expect(() =>
      parseMcpChangedNotification({
        schemaVersion: 1,
        sourceEpoch: 'not-a-process-epoch',
        sequence: 13,
        registryRevision: 7,
        kind: 'resyncRequired'
      })
    ).toThrow(/UUIDv4/)
  })
})
