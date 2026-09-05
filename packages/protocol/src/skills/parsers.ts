import {
  SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION,
  SKILL_MANAGEMENT_SCHEMA_VERSION,
  SKILL_SOURCE_RESOLUTION_SCHEMA_VERSION,
  type SkillAcquisitionSource,
  type SkillCompatibilityIssue,
  type SkillCompatibilityReport,
  type SkillDiagnostic,
  type SkillDiagnosticCode,
  type SkillGitHubReference,
  type SkillInspectionErrorData,
  type SkillInstallationChanges,
  type SkillInstallationCommitOutput,
  type SkillInstallationPreview,
  type SkillManagementActions,
  type SkillManagementEntry,
  type SkillManagementErrorData,
  type SkillPackagePreview,
  type SkillPreparationCancellationOutput,
  type SkillPreviewSource,
  type SkillResolvedGitHubRepositorySource,
  type SkillSourceDescriptor,
  type SkillSourceResolutionCandidate,
  type SkillSourceResolutionErrorData,
  type SkillsCancelSourceResolutionInput,
  type SkillsCancelSourceResolutionOutput,
  type SkillsChangedNotification,
  type SkillsListManagementOutput,
  type SkillsResolveInstallationSourceInput,
  type SkillsResolveInstallationSourceOutput,
  type SkillsSetEnabledOutput
} from './contracts'
import {
  expectArray,
  expectBoolean,
  expectCanonicalNonNilUuid,
  expectEnum,
  expectFullGitCommitSha,
  expectNonEmptyString,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  expectSchemaVersion,
  expectString,
  invalidProtocolValue,
  optionalNonEmptyString
} from './validation'

/** Validates the untrusted boundary input for read-only source resolution. */
export function parseSkillsResolveInstallationSourceInput(
  value: unknown
): SkillsResolveInstallationSourceInput {
  const record = expectRecord(value, 'Skill source resolution request')
  expectOnlyKeys(record, ['resolutionId', 'locator'] as const, 'Skill source resolution request')
  const locator = expectRecord(record.locator, 'Skill source resolution request.locator')
  expectOnlyKeys(locator, ['kind', 'url'] as const, 'Skill source resolution request.locator')
  if (locator.kind !== 'url') {
    throw invalidProtocolValue(
      'Skill source resolution request.locator',
      `unknown kind ${String(locator.kind)}`
    )
  }
  return {
    resolutionId: expectCanonicalNonNilUuid(
      record.resolutionId,
      'Skill source resolution request.resolutionId'
    ),
    locator: {
      kind: 'url',
      url: expectString(locator.url, 'Skill source resolution request.locator.url')
    }
  }
}

/** Validates the idempotent release request for retained source-resolution authority. */
export function parseSkillsCancelSourceResolutionInput(
  value: unknown
): SkillsCancelSourceResolutionInput {
  const context = 'Skill source resolution cancellation request'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['resolutionId'] as const, context)
  return {
    resolutionId: expectCanonicalNonNilUuid(record.resolutionId, `${context}.resolutionId`)
  }
}

/** Validates the authoritative result of an idempotent source-resolution cancellation. */
export function parseSkillsCancelSourceResolutionOutput(
  value: unknown
): SkillsCancelSourceResolutionOutput {
  const context = 'Skill source resolution cancellation response'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'resolutionId', 'outcome'] as const, context)
  expectSchemaVersion(record, SKILL_SOURCE_RESOLUTION_SCHEMA_VERSION, context)
  return {
    schemaVersion: SKILL_SOURCE_RESOLUTION_SCHEMA_VERSION,
    resolutionId: expectCanonicalNonNilUuid(record.resolutionId, `${context}.resolutionId`),
    outcome: expectEnum(
      record.outcome,
      ['cancelled', 'alreadyCancelled', 'alreadyConsumed', 'alreadyAbsent'] as const,
      `${context}.outcome`
    )
  }
}

/** Validates an acquisition source before it crosses a trusted application boundary. */
export function parseSkillAcquisitionSource(value: unknown): SkillAcquisitionSource {
  const context = 'Skill acquisition source'
  const record = expectRecord(value, context)
  switch (record.kind) {
    case 'localDirectory':
      expectOnlyKeys(record, ['kind', 'directory'] as const, context)
      return {
        kind: 'localDirectory',
        directory: expectNonEmptyString(record.directory, `${context}.directory`)
      }
    case 'githubRepository':
      return parseSkillGitHubAcquisitionSource(record, context)
    case 'resolvedCandidate': {
      expectOnlyKeys(record, ['kind', 'resolutionId', 'candidateId'] as const, context)
      return {
        kind: 'resolvedCandidate',
        resolutionId: expectCanonicalNonNilUuid(record.resolutionId, `${context}.resolutionId`),
        candidateId: expectNonEmptyString(record.candidateId, `${context}.candidateId`)
      }
    }
    case 'installedSource':
      expectOnlyKeys(record, ['kind'] as const, context)
      return { kind: 'installedSource' }
    default:
      throw invalidProtocolValue(context, `unknown kind ${String(record.kind)}`)
  }
}

/** Validates immutable presentation metadata and separate one-time acquisition handles. */
export function parseSkillsResolveInstallationSourceOutput(
  value: unknown
): SkillsResolveInstallationSourceOutput {
  const context = 'Skill source resolution response'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'resolutionId',
      'canonicalUrl',
      'provider',
      'resolvedCommit',
      'expiresAtUnixMs',
      'outcome',
      'candidates'
    ] as const,
    context
  )
  expectSchemaVersion(record, SKILL_SOURCE_RESOLUTION_SCHEMA_VERSION, context)
  const resolutionId = expectCanonicalNonNilUuid(record.resolutionId, `${context}.resolutionId`)
  const resolvedCommit = expectFullGitCommitSha(record.resolvedCommit, `${context}.resolvedCommit`)
  const candidates = expectArray(record.candidates, `${context}.candidates`).map((candidate) =>
    parseSkillSourceResolutionCandidate(candidate, resolutionId, resolvedCommit)
  )
  const candidateIds = new Set(candidates.map((candidate) => candidate.candidateId))
  if (candidateIds.size !== candidates.length) {
    throw invalidProtocolValue(context, 'candidateId values must be unique')
  }

  const base = {
    schemaVersion: SKILL_SOURCE_RESOLUTION_SCHEMA_VERSION,
    resolutionId,
    canonicalUrl: expectNonEmptyString(record.canonicalUrl, `${context}.canonicalUrl`),
    provider: expectEnum(record.provider, ['github'] as const, `${context}.provider`),
    resolvedCommit,
    expiresAtUnixMs: expectSafeInteger(record.expiresAtUnixMs, `${context}.expiresAtUnixMs`, 1)
  }
  const outcome = expectEnum(
    record.outcome,
    ['resolved', 'selectionRequired'] as const,
    `${context}.outcome`
  )
  if (outcome === 'resolved') {
    const candidate = candidates[0]
    if (candidates.length !== 1 || candidate === undefined) {
      throw invalidProtocolValue(context, 'resolved outcome requires exactly one candidate')
    }
    return { ...base, outcome, candidates: [candidate] }
  }

  const firstCandidate = candidates[0]
  const secondCandidate = candidates[1]
  if (firstCandidate === undefined || secondCandidate === undefined) {
    throw invalidProtocolValue(
      context,
      'selectionRequired outcome requires at least two candidates'
    )
  }
  return {
    ...base,
    outcome,
    candidates: [firstCandidate, secondCandidate, ...candidates.slice(2)]
  }
}

export function parseSkillSourceResolutionErrorData(
  value: unknown
): SkillSourceResolutionErrorData {
  const context = 'Skill source resolution error data'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['type', 'phase', 'code', 'recovery', 'message', 'provider', 'retryAfterMs'] as const,
    context
  )
  if (record.type !== 'skillSourceResolution') {
    throw invalidProtocolValue(context, 'type must be skillSourceResolution')
  }
  const provider =
    record.provider === undefined
      ? undefined
      : expectEnum(record.provider, ['github'] as const, `${context}.provider`)
  const retryAfterMs =
    record.retryAfterMs === undefined
      ? undefined
      : expectSafeInteger(record.retryAfterMs, `${context}.retryAfterMs`, 0)
  return {
    type: 'skillSourceResolution',
    phase: expectEnum(record.phase, ['parse', 'resolve', 'discover'] as const, `${context}.phase`),
    code: expectEnum(
      record.code,
      [
        'invalidLocator',
        'unsupportedLocator',
        'unsupportedHost',
        'unsupportedUrlShape',
        'repositoryNotFound',
        'referenceNotFound',
        'pathNotFound',
        'ambiguousReference',
        'noSkillsFound',
        'tooManySkills',
        'networkUnavailable',
        'rateLimited',
        'repositoryTooLarge',
        'unsafePackage',
        'invalidPackage',
        'resolutionIdConflict',
        'resolutionNotFoundOrExpired',
        'resolutionConsumed',
        'candidateNotFound',
        'capacityExceeded',
        'cancelled',
        'unavailable'
      ] as const,
      `${context}.code`
    ),
    recovery: expectEnum(
      record.recovery,
      [
        'fixLocator',
        'retryLater',
        'narrowLocator',
        'chooseDifferentSource',
        'retrySameResolution',
        'startNewResolution'
      ] as const,
      `${context}.recovery`
    ),
    message: expectNonEmptyString(record.message, `${context}.message`),
    ...(provider === undefined ? {} : { provider }),
    ...(retryAfterMs === undefined ? {} : { retryAfterMs })
  }
}

/** Validates a persisted installation preview before UI code may present or commit it. */
export function parseSkillInstallationPreview(value: unknown): SkillInstallationPreview {
  const record = expectRecord(value, 'Skill installation preview')
  expectSchemaVersion(
    record,
    SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION,
    'Skill installation preview'
  )
  const operation = expectEnum(
    record.operation,
    ['install', 'update'] as const,
    'Skill installation preview.operation'
  )
  return {
    schemaVersion: SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION,
    preparationId: expectNonEmptyString(
      record.preparationId,
      'Skill installation preview.preparationId'
    ),
    previewRevision: expectNonEmptyString(
      record.previewRevision,
      'Skill installation preview.previewRevision'
    ),
    operation,
    installationId: expectNonEmptyString(
      record.installationId,
      'Skill installation preview.installationId'
    ),
    skillId: expectNonEmptyString(record.skillId, 'Skill installation preview.skillId'),
    package: parseSkillPackagePreview(record.package),
    source: parseSkillPreviewSource(record.source),
    compatibility: parseSkillCompatibilityReport(record.compatibility),
    changes: parseSkillInstallationChanges(record.changes),
    expiresAtUnixMs: expectSafeInteger(
      record.expiresAtUnixMs,
      'Skill installation preview.expiresAtUnixMs',
      1
    )
  }
}

/** Validates the exact result of committing one frozen installation preview. */
export function parseSkillInstallationCommitOutput(value: unknown): SkillInstallationCommitOutput {
  const record = expectRecord(value, 'Skill installation commit response')
  expectSchemaVersion(
    record,
    SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION,
    'Skill installation commit response'
  )
  const operation = expectEnum(
    record.operation,
    ['install', 'update'] as const,
    'Skill installation commit response.operation'
  )
  const outcome = expectEnum(
    record.outcome,
    ['installed', 'alreadyInstalled', 'updated', 'alreadyCurrent'] as const,
    'Skill installation commit response.outcome'
  )
  if (
    (operation === 'install' && outcome !== 'installed' && outcome !== 'alreadyInstalled') ||
    (operation === 'update' && outcome !== 'updated' && outcome !== 'alreadyCurrent')
  ) {
    throw invalidProtocolValue(
      'Skill installation commit response',
      `outcome ${outcome} is invalid for ${operation}`
    )
  }
  return {
    schemaVersion: SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION,
    preparationId: expectNonEmptyString(
      record.preparationId,
      'Skill installation commit response.preparationId'
    ),
    operation,
    outcome,
    installationId: expectNonEmptyString(
      record.installationId,
      'Skill installation commit response.installationId'
    ),
    skillId: expectNonEmptyString(record.skillId, 'Skill installation commit response.skillId'),
    packageRevision: expectNonEmptyString(
      record.packageRevision,
      'Skill installation commit response.packageRevision'
    ),
    installationRevision: expectNonEmptyString(
      record.installationRevision,
      'Skill installation commit response.installationRevision'
    ),
    changes: parseSkillInstallationChanges(record.changes)
  }
}

export function parseSkillPreparationCancellationOutput(
  value: unknown
): SkillPreparationCancellationOutput {
  const record = expectRecord(value, 'Skill preparation cancellation response')
  expectSchemaVersion(
    record,
    SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION,
    'Skill preparation cancellation response'
  )
  return {
    schemaVersion: SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION,
    preparationId: expectNonEmptyString(
      record.preparationId,
      'Skill preparation cancellation response.preparationId'
    ),
    outcome: expectEnum(
      record.outcome,
      ['cancelled', 'alreadyAbsent'] as const,
      'Skill preparation cancellation response.outcome'
    )
  }
}

export function parseSkillsListManagementOutput(value: unknown): SkillsListManagementOutput {
  const record = expectRecord(value, 'Skill management list response')
  expectSchemaVersion(record, SKILL_MANAGEMENT_SCHEMA_VERSION, 'Skill management list response')
  return {
    schemaVersion: SKILL_MANAGEMENT_SCHEMA_VERSION,
    managementRevision: expectNonEmptyString(
      record.managementRevision,
      'Skill management list response.managementRevision'
    ),
    skills: expectArray(record.skills, 'Skill management list response.skills').map(
      parseSkillManagementEntry
    ),
    diagnostics: expectArray(record.diagnostics, 'Skill management list response.diagnostics').map(
      parseSkillDiagnostic
    ),
    truncated: expectBoolean(record.truncated, 'Skill management list response.truncated')
  }
}

export function parseSkillsSetEnabledOutput(value: unknown): SkillsSetEnabledOutput {
  const record = expectRecord(value, 'Skill enablement response')
  expectSchemaVersion(record, SKILL_MANAGEMENT_SCHEMA_VERSION, 'Skill enablement response')
  return {
    schemaVersion: SKILL_MANAGEMENT_SCHEMA_VERSION,
    managementRevision: expectNonEmptyString(
      record.managementRevision,
      'Skill enablement response.managementRevision'
    ),
    skillId: expectNonEmptyString(record.skillId, 'Skill enablement response.skillId'),
    stateRevision: expectNonEmptyString(
      record.stateRevision,
      'Skill enablement response.stateRevision'
    ),
    enabled: expectBoolean(record.enabled, 'Skill enablement response.enabled'),
    outcome: expectEnum(
      record.outcome,
      ['updated', 'alreadyCurrent'] as const,
      'Skill enablement response.outcome'
    )
  }
}

export function parseSkillsChangedNotification(value: unknown): SkillsChangedNotification {
  const record = expectRecord(value, 'Skills changed notification')
  expectSchemaVersion(record, SKILL_MANAGEMENT_SCHEMA_VERSION, 'Skills changed notification')
  const skillId = optionalNonEmptyString(record.skillId, 'Skills changed notification.skillId')
  return {
    schemaVersion: SKILL_MANAGEMENT_SCHEMA_VERSION,
    managementRevision: expectNonEmptyString(
      record.managementRevision,
      'Skills changed notification.managementRevision'
    ),
    reason: expectEnum(
      record.reason,
      ['installed', 'updated', 'uninstalled', 'enablementChanged', 'catalogChanged'] as const,
      'Skills changed notification.reason'
    ),
    ...(skillId === undefined ? {} : { skillId })
  }
}

export function parseSkillInspectionErrorData(value: unknown): SkillInspectionErrorData {
  const record = expectRecord(value, 'Skill inspection error data')
  if (record.type !== 'skillInspection') {
    throw invalidProtocolValue('Skill inspection error data', 'type must be skillInspection')
  }
  const preparationId = optionalNonEmptyString(
    record.preparationId,
    'Skill inspection error data.preparationId'
  )
  const diagnosticCode = optionalNonEmptyString(
    record.diagnosticCode,
    'Skill inspection error data.diagnosticCode'
  )
  const retryAfterMs =
    record.retryAfterMs === undefined
      ? undefined
      : expectSafeInteger(record.retryAfterMs, 'Skill inspection error data.retryAfterMs', 0)
  const commitMayHaveSucceeded =
    record.commitMayHaveSucceeded === undefined
      ? undefined
      : record.commitMayHaveSucceeded === true
        ? true
        : (() => {
            throw invalidProtocolValue(
              'Skill inspection error data.commitMayHaveSucceeded',
              'must be true when present'
            )
          })()
  const skillId = optionalNonEmptyString(record.skillId, 'Skill inspection error data.skillId')
  const intendedInstallationRevision = optionalNonEmptyString(
    record.intendedInstallationRevision,
    'Skill inspection error data.intendedInstallationRevision'
  )
  if (
    commitMayHaveSucceeded === true &&
    (record.phase !== 'commit' || record.code !== 'commitIndeterminate')
  ) {
    throw invalidProtocolValue(
      'Skill inspection error data',
      'commitMayHaveSucceeded requires a commitIndeterminate commit error'
    )
  }
  return {
    type: 'skillInspection',
    phase: expectEnum(
      record.phase,
      ['inspect', 'commit', 'cancel'] as const,
      'Skill inspection error data.phase'
    ),
    code: expectEnum(
      record.code,
      [
        'invalidSource',
        'unsupportedSource',
        'sourceNotAccessible',
        'referenceNotFound',
        'subdirectoryNotFound',
        'sourceChangedDuringRead',
        'resolutionNotFound',
        'resolutionExpired',
        'resolutionConsumed',
        'candidateNotFound',
        'capacityExceeded',
        'installationRetired',
        'installationNotFound',
        'installationRevisionConflict',
        'sourceNotRefreshable',
        'persistedSourceInvalid',
        'networkUnavailable',
        'rateLimited',
        'repositoryTooLarge',
        'unsafePackage',
        'invalidPackage',
        'incompatible',
        'preparationNotFound',
        'preparationExpired',
        'idempotencyConflict',
        'previewMismatch',
        'acknowledgementRequired',
        'commitIndeterminate',
        'cancelled',
        'unavailable'
      ] as const,
      'Skill inspection error data.code'
    ),
    recovery: expectEnum(
      record.recovery,
      [
        'fixSource',
        'retrySamePreparation',
        'retryLater',
        'inspectAgain',
        'newInstallationIdentity',
        'freeCapacity',
        'contactSupport',
        'acknowledgeWarnings',
        'chooseDifferentSource',
        'resolveAgain',
        'refreshManagement'
      ] as const,
      'Skill inspection error data.recovery'
    ),
    message: expectNonEmptyString(record.message, 'Skill inspection error data.message'),
    ...(preparationId === undefined ? {} : { preparationId }),
    ...(diagnosticCode === undefined ? {} : { diagnosticCode }),
    ...(retryAfterMs === undefined ? {} : { retryAfterMs }),
    ...(commitMayHaveSucceeded === undefined ? {} : { commitMayHaveSucceeded }),
    ...(skillId === undefined ? {} : { skillId }),
    ...(intendedInstallationRevision === undefined ? {} : { intendedInstallationRevision })
  }
}

export function parseSkillManagementErrorData(value: unknown): SkillManagementErrorData {
  const record = expectRecord(value, 'Skill management error data')
  expectOnlyKeys(
    record,
    ['type', 'operation', 'code', 'recovery', 'message'] as const,
    'Skill management error data'
  )
  if (record.type !== 'skillManagement') {
    throw invalidProtocolValue('Skill management error data', 'type must be skillManagement')
  }
  return {
    type: 'skillManagement',
    operation: expectEnum(
      record.operation,
      ['list', 'setEnabled'] as const,
      'Skill management error data.operation'
    ),
    code: expectEnum(
      record.code,
      [
        'notFound',
        'notManageable',
        'stateConflict',
        'configurationRequired',
        'storageUnavailable'
      ] as const,
      'Skill management error data.code'
    ),
    recovery: expectEnum(
      record.recovery,
      ['refreshManagement', 'configureImageGeneration', 'retry'] as const,
      'Skill management error data.recovery'
    ),
    message: expectNonEmptyString(record.message, 'Skill management error data.message')
  }
}

function parseSkillPackagePreview(value: unknown): SkillPackagePreview {
  const record = expectRecord(value, 'Skill package preview')
  return {
    formatVersion: expectSafeInteger(
      record.formatVersion,
      'Skill package preview.formatVersion',
      1
    ),
    packageRevision: expectNonEmptyString(
      record.packageRevision,
      'Skill package preview.packageRevision'
    ),
    name: expectNonEmptyString(record.name, 'Skill package preview.name'),
    description: expectNonEmptyString(record.description, 'Skill package preview.description'),
    fileCount: expectSafeInteger(record.fileCount, 'Skill package preview.fileCount', 1),
    totalBytes: expectSafeInteger(record.totalBytes, 'Skill package preview.totalBytes', 1)
  }
}

function parseSkillSourceResolutionCandidate(
  value: unknown,
  resolutionId: string,
  resolvedCommit: string
): SkillSourceResolutionCandidate {
  const context = 'Skill source resolution candidate'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['candidateId', 'acquisition', 'source', 'package'] as const, context)
  const candidateId = expectNonEmptyString(record.candidateId, `${context}.candidateId`)
  const acquisitionContext = `${context}.acquisition`
  const acquisition = expectRecord(record.acquisition, acquisitionContext)
  expectOnlyKeys(acquisition, ['kind', 'resolutionId', 'candidateId'] as const, acquisitionContext)
  if (acquisition.kind !== 'resolvedCandidate') {
    throw invalidProtocolValue(acquisitionContext, 'kind must be resolvedCandidate')
  }
  const acquisitionResolutionId = expectCanonicalNonNilUuid(
    acquisition.resolutionId,
    `${acquisitionContext}.resolutionId`
  )
  if (acquisitionResolutionId !== resolutionId) {
    throw invalidProtocolValue(acquisitionContext, 'resolutionId must match response.resolutionId')
  }
  const acquisitionCandidateId = expectNonEmptyString(
    acquisition.candidateId,
    `${acquisitionContext}.candidateId`
  )
  if (acquisitionCandidateId !== candidateId) {
    throw invalidProtocolValue(
      acquisitionContext,
      'candidateId must match the enclosing candidateId'
    )
  }
  return {
    candidateId,
    acquisition: {
      kind: 'resolvedCandidate',
      resolutionId: acquisitionResolutionId,
      candidateId: acquisitionCandidateId
    },
    source: parseSkillResolvedGitHubRepositorySource(record.source, resolvedCommit),
    package: parseSkillSourceResolutionPackagePreview(record.package)
  }
}

function parseSkillResolvedGitHubRepositorySource(
  value: unknown,
  resolvedCommit: string
): SkillResolvedGitHubRepositorySource {
  const context = 'Skill source resolution candidate.source'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['kind', 'owner', 'repository', 'reference', 'resolvedCommit', 'subdirectory'] as const,
    context
  )
  if (record.kind !== 'githubRepository') {
    throw invalidProtocolValue(context, 'kind must be githubRepository')
  }
  const candidateResolvedCommit = expectFullGitCommitSha(
    record.resolvedCommit,
    `${context}.resolvedCommit`
  )
  if (candidateResolvedCommit !== resolvedCommit) {
    throw invalidProtocolValue(context, 'resolvedCommit must match response.resolvedCommit')
  }
  const reference = parseSkillGitHubReference(record.reference, `${context}.reference`)
  if (reference.kind === 'commit' && reference.sha !== candidateResolvedCommit) {
    throw invalidProtocolValue(context, 'commit reference sha must match resolvedCommit')
  }
  const subdirectory = optionalNonEmptyString(record.subdirectory, `${context}.subdirectory`)
  return {
    kind: 'githubRepository',
    owner: expectNonEmptyString(record.owner, `${context}.owner`),
    repository: expectNonEmptyString(record.repository, `${context}.repository`),
    reference,
    resolvedCommit: candidateResolvedCommit,
    ...(subdirectory === undefined ? {} : { subdirectory })
  }
}

function parseSkillGitHubAcquisitionSource(
  record: Record<string, unknown>,
  context: string
): Extract<SkillAcquisitionSource, { kind: 'githubRepository' }> {
  expectOnlyKeys(
    record,
    ['kind', 'owner', 'repository', 'reference', 'subdirectory'] as const,
    context
  )
  if (record.kind !== 'githubRepository') {
    throw invalidProtocolValue(context, 'kind must be githubRepository')
  }
  const reference =
    record.reference === undefined
      ? undefined
      : parseSkillGitHubReference(record.reference, `${context}.reference`)
  const subdirectory = optionalNonEmptyString(record.subdirectory, `${context}.subdirectory`)
  return {
    kind: 'githubRepository',
    owner: expectNonEmptyString(record.owner, `${context}.owner`),
    repository: expectNonEmptyString(record.repository, `${context}.repository`),
    ...(reference === undefined ? {} : { reference }),
    ...(subdirectory === undefined ? {} : { subdirectory })
  }
}

function parseSkillGitHubReference(
  value: unknown,
  context = 'Skill GitHub reference'
): SkillGitHubReference {
  const record = expectRecord(value, context)
  switch (record.kind) {
    case 'defaultBranch':
      expectOnlyKeys(record, ['kind'] as const, context)
      return { kind: 'defaultBranch' }
    case 'named':
      expectOnlyKeys(record, ['kind', 'value'] as const, context)
      return {
        kind: 'named',
        value: expectNonEmptyString(record.value, `${context}.value`)
      }
    case 'commit':
      expectOnlyKeys(record, ['kind', 'sha'] as const, context)
      return {
        kind: 'commit',
        sha: expectFullGitCommitSha(record.sha, `${context}.sha`)
      }
    default:
      throw invalidProtocolValue(context, `unknown kind ${String(record.kind)}`)
  }
}

function parseSkillSourceResolutionPackagePreview(value: unknown): SkillPackagePreview {
  const context = 'Skill source resolution candidate.package'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['formatVersion', 'packageRevision', 'name', 'description', 'fileCount', 'totalBytes'] as const,
    context
  )
  return parseSkillPackagePreview(record)
}

function parseSkillPreviewSource(value: unknown): SkillPreviewSource {
  const record = expectRecord(value, 'Skill preview source')
  switch (record.kind) {
    case 'localDirectory':
      return {
        kind: 'localDirectory',
        displayName: expectNonEmptyString(record.displayName, 'Skill preview source.displayName'),
        refreshable: expectBoolean(record.refreshable, 'Skill preview source.refreshable')
      }
    case 'githubRepository': {
      const subdirectory = optionalNonEmptyString(
        record.subdirectory,
        'Skill preview source.subdirectory'
      )
      return {
        kind: 'githubRepository',
        owner: expectNonEmptyString(record.owner, 'Skill preview source.owner'),
        repository: expectNonEmptyString(record.repository, 'Skill preview source.repository'),
        reference: parseSkillGitHubReference(record.reference),
        resolvedCommit: expectFullGitCommitSha(
          record.resolvedCommit,
          'Skill preview source.resolvedCommit'
        ),
        ...(subdirectory === undefined ? {} : { subdirectory }),
        refreshable: expectBoolean(record.refreshable, 'Skill preview source.refreshable')
      }
    }
    case 'installedSource':
      return {
        kind: 'installedSource',
        displayName: expectNonEmptyString(record.displayName, 'Skill preview source.displayName'),
        refreshable: expectBoolean(record.refreshable, 'Skill preview source.refreshable')
      }
    default:
      throw invalidProtocolValue('Skill preview source', `unknown kind ${String(record.kind)}`)
  }
}

function parseSkillCompatibilityReport(value: unknown): SkillCompatibilityReport {
  const record = expectRecord(value, 'Skill compatibility report')
  return {
    status: expectEnum(
      record.status,
      ['compatible', 'compatibleWithWarnings', 'unknown', 'incompatible'] as const,
      'Skill compatibility report.status'
    ),
    issues: expectArray(record.issues, 'Skill compatibility report.issues').map(
      parseSkillCompatibilityIssue
    )
  }
}

function parseSkillCompatibilityIssue(value: unknown): SkillCompatibilityIssue {
  const record = expectRecord(value, 'Skill compatibility issue')
  const capability = optionalNonEmptyString(
    record.capability,
    'Skill compatibility issue.capability'
  )
  return {
    id: expectNonEmptyString(record.id, 'Skill compatibility issue.id'),
    code: expectNonEmptyString(record.code, 'Skill compatibility issue.code'),
    severity: expectEnum(
      record.severity,
      ['warning', 'error'] as const,
      'Skill compatibility issue.severity'
    ),
    message: expectNonEmptyString(record.message, 'Skill compatibility issue.message'),
    ...(capability === undefined ? {} : { capability }),
    requiresAcknowledgement: expectBoolean(
      record.requiresAcknowledgement,
      'Skill compatibility issue.requiresAcknowledgement'
    )
  }
}

function parseSkillInstallationChanges(value: unknown): SkillInstallationChanges {
  const record = expectRecord(value, 'Skill installation changes')
  return {
    content: expectEnum(
      record.content,
      ['new', 'changed', 'unchanged'] as const,
      'Skill installation changes.content'
    ),
    source: expectEnum(
      record.source,
      ['new', 'changed', 'unchanged'] as const,
      'Skill installation changes.source'
    )
  }
}

function parseSkillManagementEntry(value: unknown): SkillManagementEntry {
  const record = expectRecord(value, 'Skill management entry')
  const installationRevision = optionalNonEmptyString(
    record.installationRevision,
    'Skill management entry.installationRevision'
  )
  const acquisition =
    record.acquisition === undefined ? undefined : parseSkillPreviewSource(record.acquisition)
  return {
    id: expectNonEmptyString(record.id, 'Skill management entry.id'),
    name: expectNonEmptyString(record.name, 'Skill management entry.name'),
    description: expectNonEmptyString(record.description, 'Skill management entry.description'),
    source: parseSkillSourceDescriptor(record.source),
    packageRevision: expectNonEmptyString(
      record.packageRevision,
      'Skill management entry.packageRevision'
    ),
    ...(installationRevision === undefined ? {} : { installationRevision }),
    stateRevision: expectNonEmptyString(
      record.stateRevision,
      'Skill management entry.stateRevision'
    ),
    enabled: expectBoolean(record.enabled, 'Skill management entry.enabled'),
    ...(record.enablementBlock === undefined
      ? {}
      : {
          enablementBlock: expectEnum(
            record.enablementBlock,
            ['imageGenerationConfigurationRequired'] as const,
            'Skill management entry.enablementBlock'
          )
        }),
    actions: parseSkillManagementActions(record.actions),
    ...(acquisition === undefined ? {} : { acquisition }),
    compatibility: parseSkillCompatibilityReport(record.compatibility)
  }
}

function parseSkillManagementActions(value: unknown): SkillManagementActions {
  const record = expectRecord(value, 'Skill management actions')
  return {
    canSetEnabled: expectBoolean(record.canSetEnabled, 'Skill management actions.canSetEnabled'),
    canUpdate: expectBoolean(record.canUpdate, 'Skill management actions.canUpdate'),
    canUninstall: expectBoolean(record.canUninstall, 'Skill management actions.canUninstall')
  }
}

function parseSkillSourceDescriptor(value: unknown): SkillSourceDescriptor {
  const record = expectRecord(value, 'Skill source descriptor')
  const id = expectNonEmptyString(record.id, 'Skill source descriptor.id')
  switch (record.kind) {
    case 'workspace':
      return { kind: 'workspace', id }
    case 'bundled':
      return { kind: 'bundled', id }
    case 'installed':
      return { kind: 'installed', id }
    default:
      throw invalidProtocolValue('Skill source descriptor', `unknown kind ${String(record.kind)}`)
  }
}

function parseSkillDiagnostic(value: unknown): SkillDiagnostic {
  const record = expectRecord(value, 'Skill diagnostic')
  expectOnlyKeys(
    record,
    ['code', 'severity', 'message', 'skillId', 'location'] as const,
    'Skill diagnostic'
  )
  const skillId = optionalNonEmptyString(record.skillId, 'Skill diagnostic.skillId')
  const location = optionalNonEmptyString(record.location, 'Skill diagnostic.location')
  return {
    code: expectEnum(
      record.code,
      [
        'invalidRoot',
        'rootEscapesWorkspace',
        'tooManyEntries',
        'scanBudgetExceeded',
        'catalogTooLarge',
        'unreadableEntry',
        'unsupportedPathEncoding',
        'symlinkNotAllowed',
        'pathChangedDuringRead',
        'missingSkillFile',
        'skillFileTooLarge',
        'invalidUtf8',
        'nulByte',
        'missingFrontmatter',
        'invalidFrontmatter',
        'missingDescription',
        'invalidName',
        'invalidDescription',
        'invalidDirectoryName',
        'missingInstructions',
        'defaultedName',
        'duplicateName',
        'unsupportedToolReference',
        'invalidInstallationReceipt',
        'packageRevisionMismatch',
        'unexpectedPackageEntry',
        'invalidResourcePath',
        'resourceFileTooLarge',
        'packageTooLarge',
        'invalidPackageManifest',
        'sourceUnavailable',
        'sourceContractViolation'
      ] as const satisfies readonly SkillDiagnosticCode[],
      'Skill diagnostic.code'
    ),
    severity: expectEnum(
      record.severity,
      ['warning', 'error'] as const,
      'Skill diagnostic.severity'
    ),
    message: expectNonEmptyString(record.message, 'Skill diagnostic.message'),
    ...(skillId === undefined ? {} : { skillId }),
    ...(location === undefined ? {} : { location })
  }
}
