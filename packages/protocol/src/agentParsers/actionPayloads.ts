import type {
  AgentCommandActionProjection,
  AgentOfficeOperationRequest,
  AgentSkillInstallationRequest,
  AgentSkillMaterializationRequest,
  AgentSkillScriptRequest
} from '../agent'
import {
  expectBoolean,
  expectEnum,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  invalidProtocolValue
} from '../skills/validation'
import {
  MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES,
  assertRendererSafeJson,
  expectBoundedArray,
  expectBoundedNonEmptyString,
  expectBoundedString,
  expectExactSchemaVersion,
  expectOpaqueRunId,
  parseStringArray
} from './shared'
export function parseAgentApprovalStatus(value: unknown, context: string) {
  return expectEnum(value, ['not_required', 'required', 'approved', 'rejected'] as const, context)
}

export function parseAgentCommandAction(
  value: unknown,
  context: string
): AgentCommandActionProjection {
  const item = expectRecord(value, context)
  expectOnlyKeys(
    item,
    [
      'id',
      'command',
      'cwd',
      'timeoutMs',
      'approvalStatus',
      'riskLevel',
      'reason',
      'observe'
    ] as const,
    context
  )
  const observe =
    item.observe === null
      ? null
      : parseAgentCommandObservationRequest(item.observe, `${context}.observe`)
  return {
    id: expectOpaqueRunId(item.id, `${context}.id`),
    command: expectBoundedString(item.command, `${context}.command`, 256 * 1024),
    cwd: item.cwd === null ? null : expectBoundedString(item.cwd, `${context}.cwd`, 16 * 1024),
    timeoutMs:
      item.timeoutMs === null ? null : expectSafeInteger(item.timeoutMs, `${context}.timeoutMs`, 0),
    approvalStatus: parseAgentApprovalStatus(item.approvalStatus, `${context}.approvalStatus`),
    riskLevel:
      item.riskLevel === null
        ? null
        : expectEnum(
            item.riskLevel,
            ['read_only', 'writes_workspace', 'network', 'destructive', 'unknown'] as const,
            `${context}.riskLevel`
          ),
    reason:
      item.reason === null
        ? null
        : expectBoundedString(item.reason, `${context}.reason`, 16 * 1024),
    observe
  }
}

export function parseAgentCommandObservationRequest(
  value: unknown,
  context: string
): NonNullable<AgentCommandActionProjection['observe']> {
  const item = expectRecord(value, context)
  expectOnlyKeys(item, ['kinds', 'expectedOutputs', 'additionalRoots'] as const, context)
  const kinds = expectBoundedArray(item.kinds, `${context}.kinds`, 32).map((kind, index) =>
    expectEnum(kind, ['office'] as const, `${context}.kinds[${index}]`)
  )
  return {
    kinds,
    ...(item.expectedOutputs === undefined
      ? {}
      : {
          expectedOutputs: parseStringArray(
            item.expectedOutputs,
            `${context}.expectedOutputs`,
            1024,
            16 * 1024
          )
        }),
    ...(item.additionalRoots === undefined
      ? {}
      : {
          additionalRoots: parseStringArray(
            item.additionalRoots,
            `${context}.additionalRoots`,
            1024,
            16 * 1024
          )
        })
  }
}

export function parseAgentSkillMaterializationRequest(
  value: unknown,
  context: string
): AgentSkillMaterializationRequest {
  const item = expectRecord(value, context)
  expectOnlyKeys(
    item,
    ['id', 'sourceUri', 'sourcePrefix', 'destination', 'approvalStatus', 'reason'] as const,
    context
  )
  return {
    id: expectOpaqueRunId(item.id, `${context}.id`),
    sourceUri: expectBoundedString(item.sourceUri, `${context}.sourceUri`, 16 * 1024),
    sourcePrefix:
      item.sourcePrefix === null
        ? null
        : expectBoundedString(item.sourcePrefix, `${context}.sourcePrefix`, 16 * 1024),
    destination: expectBoundedString(item.destination, `${context}.destination`, 16 * 1024),
    approvalStatus: parseAgentApprovalStatus(item.approvalStatus, `${context}.approvalStatus`),
    reason:
      item.reason === null ? null : expectBoundedString(item.reason, `${context}.reason`, 16 * 1024)
  }
}

export function parseAgentSkillScriptRequest(
  value: unknown,
  context: string
): AgentSkillScriptRequest {
  const item = expectRecord(value, context)
  expectOnlyKeys(
    item,
    [
      'id',
      'scriptUri',
      'skillId',
      'skillRevision',
      'resourcePath',
      'resourceDigest',
      'source',
      'interpreter',
      'args',
      'requirements',
      'preflight',
      'timeoutMs',
      'approvalStatus',
      'reason'
    ] as const,
    context
  )
  return {
    id: expectOpaqueRunId(item.id, `${context}.id`),
    scriptUri: expectBoundedString(item.scriptUri, `${context}.scriptUri`, 16 * 1024),
    skillId: expectOpaqueRunId(item.skillId, `${context}.skillId`),
    skillRevision: expectOpaqueRunId(item.skillRevision, `${context}.skillRevision`),
    resourcePath: expectBoundedString(item.resourcePath, `${context}.resourcePath`, 16 * 1024),
    resourceDigest: expectBoundedNonEmptyString(
      item.resourceDigest,
      `${context}.resourceDigest`,
      2048
    ),
    source: parseAgentSkillScriptSourceProof(item.source, `${context}.source`),
    interpreter: expectEnum(item.interpreter, ['python3'] as const, `${context}.interpreter`),
    args: parseStringArray(item.args, `${context}.args`, 4096, 64 * 1024),
    requirements: parseAgentSkillScriptRequirements(item.requirements, `${context}.requirements`),
    preflight: parseAgentSkillScriptPreflight(item.preflight, `${context}.preflight`),
    timeoutMs:
      item.timeoutMs === null ? null : expectSafeInteger(item.timeoutMs, `${context}.timeoutMs`, 0),
    approvalStatus: parseAgentApprovalStatus(item.approvalStatus, `${context}.approvalStatus`),
    reason:
      item.reason === null ? null : expectBoundedString(item.reason, `${context}.reason`, 16 * 1024)
  }
}

export function parseAgentSkillScriptSourceProof(
  value: unknown,
  context: string
): AgentSkillScriptRequest['source'] {
  const item = expectRecord(value, context)
  expectOnlyKeys(item, ['sourceId', 'sourceKind', 'trust'] as const, context)
  return {
    sourceId: expectBoundedNonEmptyString(item.sourceId, `${context}.sourceId`, 2048),
    sourceKind: expectEnum(
      item.sourceKind,
      ['workspace', 'bundled', 'installed'] as const,
      `${context}.sourceKind`
    ),
    trust: expectEnum(
      item.trust,
      ['untrusted', 'user_approved', 'application'] as const,
      `${context}.trust`
    )
  }
}

export function parseAgentSkillScriptRequirements(
  value: unknown,
  context: string
): AgentSkillScriptRequest['requirements'] {
  const item = expectRecord(value, context)
  expectOnlyKeys(item, ['pythonDistributions', 'commands'] as const, context)
  return {
    ...(item.pythonDistributions === undefined
      ? {}
      : {
          pythonDistributions: parseStringArray(
            item.pythonDistributions,
            `${context}.pythonDistributions`,
            4096,
            1024
          )
        }),
    ...(item.commands === undefined
      ? {}
      : {
          commands: parseStringArray(item.commands, `${context}.commands`, 4096, 1024)
        })
  }
}

export function parseAgentSkillScriptPreflight(
  value: unknown,
  context: string
): AgentSkillScriptRequest['preflight'] {
  const item = expectRecord(value, context)
  expectOnlyKeys(
    item,
    [
      'status',
      'interpreter',
      'interpreterVersion',
      'dependencies',
      'runtimeFingerprint',
      'errorCode',
      'message'
    ] as const,
    context
  )
  return {
    status: expectEnum(
      item.status,
      ['ready', 'missing_dependencies', 'unsupported', 'conflict'] as const,
      `${context}.status`
    ),
    interpreter: expectEnum(item.interpreter, ['python3'] as const, `${context}.interpreter`),
    ...(item.interpreterVersion === undefined
      ? {}
      : {
          interpreterVersion: expectBoundedNonEmptyString(
            item.interpreterVersion,
            `${context}.interpreterVersion`,
            1024
          )
        }),
    ...(item.dependencies === undefined
      ? {}
      : {
          dependencies: expectBoundedArray(item.dependencies, `${context}.dependencies`, 4096).map(
            (dependency, index) => {
              const dependencyContext = `${context}.dependencies[${index}]`
              const entry = expectRecord(dependency, dependencyContext)
              expectOnlyKeys(
                entry,
                ['kind', 'name', 'status', 'version'] as const,
                dependencyContext
              )
              return {
                kind: expectEnum(
                  entry.kind,
                  ['python_distribution', 'command'] as const,
                  `${dependencyContext}.kind`
                ),
                name: expectBoundedNonEmptyString(entry.name, `${dependencyContext}.name`, 1024),
                status: expectEnum(
                  entry.status,
                  ['available', 'missing'] as const,
                  `${dependencyContext}.status`
                ),
                ...(entry.version === undefined
                  ? {}
                  : {
                      version: expectBoundedNonEmptyString(
                        entry.version,
                        `${dependencyContext}.version`,
                        1024
                      )
                    })
              }
            }
          )
        }),
    runtimeFingerprint: expectBoundedNonEmptyString(
      item.runtimeFingerprint,
      `${context}.runtimeFingerprint`,
      4096
    ),
    ...(item.errorCode === undefined
      ? {}
      : {
          errorCode: expectBoundedNonEmptyString(item.errorCode, `${context}.errorCode`, 1024)
        }),
    ...(item.message === undefined
      ? {}
      : {
          message: expectBoundedString(
            item.message,
            `${context}.message`,
            MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES
          )
        })
  }
}

export function parseAgentOfficeOperationRequest(
  value: unknown,
  context: string
): AgentOfficeOperationRequest {
  const item = expectRecord(value, context)
  expectOnlyKeys(
    item,
    ['schemaVersion', 'id', 'semanticArgs', 'prepared', 'approvalStatus', 'reason'] as const,
    context
  )
  const semanticArgs = expectRecord(item.semanticArgs, `${context}.semanticArgs`)
  assertRendererSafeJson(semanticArgs, `${context}.semanticArgs`, 4 * 1024 * 1024)
  return {
    schemaVersion: expectExactSchemaVersion(item.schemaVersion, 6, `${context}.schemaVersion`),
    id: expectOpaqueRunId(item.id, `${context}.id`),
    semanticArgs,
    prepared: parseAgentOfficePreparedExecution(item.prepared, `${context}.prepared`),
    approvalStatus: parseAgentApprovalStatus(item.approvalStatus, `${context}.approvalStatus`),
    reason: expectBoundedString(item.reason, `${context}.reason`, 16 * 1024)
  }
}

export function parseAgentOfficePreparedExecution(
  value: unknown,
  context: string
): AgentOfficeOperationRequest['prepared'] {
  const item = expectRecord(value, context)
  expectOnlyKeys(
    item,
    [
      'schemaVersion',
      'providerId',
      'engineRevision',
      'workspaceRevision',
      'access',
      'request',
      'argv',
      'resolvedRenderPlan',
      'paths',
      'inputBindings'
    ] as const,
    context
  )
  assertRendererSafeJson(item, context, 4 * 1024 * 1024)
  expectExactSchemaVersion(item.schemaVersion, 6, `${context}.schemaVersion`)
  expectBoundedNonEmptyString(item.providerId, `${context}.providerId`, 1024)
  expectBoundedNonEmptyString(item.engineRevision, `${context}.engineRevision`, 4096)
  if (item.workspaceRevision !== null) {
    expectBoundedNonEmptyString(item.workspaceRevision, `${context}.workspaceRevision`, 4096)
  }
  expectEnum(item.access, ['readOnly', 'fileWrite'] as const, `${context}.access`)
  parseStringArray(item.argv, `${context}.argv`, 4096, 64 * 1024)
  if (item.resolvedRenderPlan !== null) {
    expectRecord(item.resolvedRenderPlan, `${context}.resolvedRenderPlan`)
  }
  expectBoundedArray(item.paths, `${context}.paths`, 4096)
  expectBoundedArray(item.inputBindings, `${context}.inputBindings`, 4096)

  const requestContext = `${context}.request`
  const request = expectRecord(item.request, requestContext)
  expectOnlyKeys(
    request,
    [
      'documentKind',
      'operation',
      'documentPath',
      'outputPath',
      'destinationPath',
      'inputs',
      'timeoutMs',
      'parameters'
    ] as const,
    requestContext
  )
  expectEnum(
    request.documentKind,
    ['document', 'spreadsheet', 'presentation'] as const,
    `${requestContext}.documentKind`
  )
  const operation = expectEnum(
    request.operation,
    [
      'help',
      'create',
      'view',
      'get',
      'query',
      'validate',
      'set',
      'add',
      'remove',
      'move',
      'swap'
    ] as const,
    `${requestContext}.operation`
  )
  for (const field of ['documentPath', 'outputPath', 'destinationPath'] as const) {
    if (request[field] !== null) {
      expectBoundedString(request[field], `${requestContext}.${field}`, 16 * 1024)
    }
  }
  expectBoundedArray(request.inputs, `${requestContext}.inputs`, 4096)
  if (request.timeoutMs !== null) {
    expectSafeInteger(request.timeoutMs, `${requestContext}.timeoutMs`, 0)
  }
  const parameters = expectRecord(request.parameters, `${requestContext}.parameters`)
  if (parameters.type !== operation) {
    throw invalidProtocolValue(
      `${requestContext}.parameters.type`,
      'must match the prepared Office operation'
    )
  }

  return JSON.parse(JSON.stringify(item)) as AgentOfficeOperationRequest['prepared']
}

export function parseAgentSkillInstallationRequest(
  value: unknown,
  context: string
): AgentSkillInstallationRequest {
  const item = expectRecord(value, context)
  expectOnlyKeys(
    item,
    ['schemaVersion', 'id', 'installRef', 'preview', 'approvalStatus', 'expiresAt'] as const,
    context
  )
  return {
    schemaVersion: expectExactSchemaVersion(item.schemaVersion, 1, `${context}.schemaVersion`),
    id: expectOpaqueRunId(item.id, `${context}.id`),
    installRef: expectBoundedString(item.installRef, `${context}.installRef`, 16 * 1024),
    preview: parseAgentSkillInstallationPreview(item.preview, `${context}.preview`),
    approvalStatus: parseAgentApprovalStatus(item.approvalStatus, `${context}.approvalStatus`),
    expiresAt: expectSafeInteger(item.expiresAt, `${context}.expiresAt`, 0)
  }
}

export function parseAgentSkillInstallationPreview(
  value: unknown,
  context: string
): AgentSkillInstallationRequest['preview'] {
  const item = expectRecord(value, context)
  expectOnlyKeys(
    item,
    [
      'name',
      'description',
      'sourceSummary',
      'resolvedRevision',
      'fileCount',
      'totalBytes',
      'resourceSummary',
      'containsScripts',
      'warnings',
      'compatibility',
      'operation',
      'impact'
    ] as const,
    context
  )
  assertRendererSafeJson(item.sourceSummary, `${context}.sourceSummary`)
  const resourceSummary = expectRecord(item.resourceSummary, `${context}.resourceSummary`)
  expectOnlyKeys(
    resourceSummary,
    ['total', 'references', 'assets', 'scripts', 'bytes'] as const,
    `${context}.resourceSummary`
  )
  return {
    name: expectBoundedNonEmptyString(item.name, `${context}.name`, 1024),
    description: expectBoundedString(
      item.description,
      `${context}.description`,
      MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES
    ),
    sourceSummary: item.sourceSummary,
    resolvedRevision: expectBoundedNonEmptyString(
      item.resolvedRevision,
      `${context}.resolvedRevision`,
      4096
    ),
    fileCount: expectSafeInteger(item.fileCount, `${context}.fileCount`, 0),
    totalBytes: expectSafeInteger(item.totalBytes, `${context}.totalBytes`, 0),
    resourceSummary: {
      total: expectSafeInteger(resourceSummary.total, `${context}.resourceSummary.total`, 0),
      references: expectSafeInteger(
        resourceSummary.references,
        `${context}.resourceSummary.references`,
        0
      ),
      assets: expectSafeInteger(resourceSummary.assets, `${context}.resourceSummary.assets`, 0),
      scripts: expectSafeInteger(resourceSummary.scripts, `${context}.resourceSummary.scripts`, 0),
      bytes: expectSafeInteger(resourceSummary.bytes, `${context}.resourceSummary.bytes`, 0)
    },
    containsScripts: expectBoolean(item.containsScripts, `${context}.containsScripts`),
    warnings: expectBoundedArray(item.warnings, `${context}.warnings`, 1024).map(
      (warning, index) => {
        const warningContext = `${context}.warnings[${index}]`
        const entry = expectRecord(warning, warningContext)
        expectOnlyKeys(
          entry,
          ['code', 'message', 'requiresAcknowledgement'] as const,
          warningContext
        )
        return {
          code: expectBoundedNonEmptyString(entry.code, `${warningContext}.code`, 1024),
          message: expectBoundedString(
            entry.message,
            `${warningContext}.message`,
            MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES
          ),
          requiresAcknowledgement: expectBoolean(
            entry.requiresAcknowledgement,
            `${warningContext}.requiresAcknowledgement`
          )
        }
      }
    ),
    compatibility: expectBoundedNonEmptyString(
      item.compatibility,
      `${context}.compatibility`,
      1024
    ),
    operation: expectBoundedNonEmptyString(item.operation, `${context}.operation`, 1024),
    impact: expectBoundedNonEmptyString(item.impact, `${context}.impact`, 1024)
  }
}
