import type { OfficeDocumentKind, OfficeOperation } from './agent'
import {
  expectArray,
  expectBoolean,
  expectEnum,
  expectNonEmptyString,
  expectOnlyKeys,
  expectRecord,
  expectSchemaVersion,
  optionalNonEmptyString
} from './skills/validation'

export const OFFICE_GET_STATUS_METHOD = 'office.getStatus' as const
export const OFFICE_ENGINE_STATUS_SCHEMA_VERSION = 1 as const

export type OfficeEngineAvailability = 'available' | 'unavailable'
export type OfficeEngineSource = 'configured' | 'packagedComponent' | 'developmentPath'

export interface OfficeEngineCapabilities {
  providerId: string
  documentKinds: OfficeDocumentKind[]
  operations: OfficeOperation[]
  supportsRendering: boolean
  supportsValidation: boolean
  supportsStructuredOutput: boolean
}

export interface OfficeEngineStatus {
  schemaVersion: typeof OFFICE_ENGINE_STATUS_SCHEMA_VERSION
  providerId: string
  availability: OfficeEngineAvailability
  source?: OfficeEngineSource
  version?: string
  engineRevision?: string
  capabilities: OfficeEngineCapabilities
  errorCode?: string
  message?: string
}

const DOCUMENT_KINDS = ['document', 'spreadsheet', 'presentation'] as const
const OPERATIONS = [
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
] as const

function parseEnumArray<const T extends readonly string[]>(
  value: unknown,
  allowed: T,
  context: string
): T[number][] {
  return expectArray(value, context).map((entry, index) =>
    expectEnum(entry, allowed, `${context}[${index}]`)
  )
}

function parseOfficeEngineCapabilities(value: unknown): OfficeEngineCapabilities {
  const context = 'Office engine status.capabilities'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'providerId',
      'documentKinds',
      'operations',
      'supportsRendering',
      'supportsValidation',
      'supportsStructuredOutput'
    ] as const,
    context
  )

  return {
    providerId: expectNonEmptyString(record.providerId, `${context}.providerId`),
    documentKinds: parseEnumArray(record.documentKinds, DOCUMENT_KINDS, `${context}.documentKinds`),
    operations: parseEnumArray(record.operations, OPERATIONS, `${context}.operations`),
    supportsRendering: expectBoolean(record.supportsRendering, `${context}.supportsRendering`),
    supportsValidation: expectBoolean(record.supportsValidation, `${context}.supportsValidation`),
    supportsStructuredOutput: expectBoolean(
      record.supportsStructuredOutput,
      `${context}.supportsStructuredOutput`
    )
  }
}

/** Strictly validates the untrusted JSON-RPC result before it reaches application code. */
export function parseOfficeEngineStatus(value: unknown): OfficeEngineStatus {
  const context = 'Office engine status'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'providerId',
      'availability',
      'source',
      'version',
      'engineRevision',
      'capabilities',
      'errorCode',
      'message'
    ] as const,
    context
  )
  expectSchemaVersion(record, OFFICE_ENGINE_STATUS_SCHEMA_VERSION, context)

  const providerId = expectNonEmptyString(record.providerId, `${context}.providerId`)
  const capabilities = parseOfficeEngineCapabilities(record.capabilities)
  if (capabilities.providerId !== providerId) {
    throw new Error(`Invalid ${context}: capabilities provider does not match status provider`)
  }
  const version = optionalNonEmptyString(record.version, `${context}.version`)
  const engineRevision = optionalNonEmptyString(record.engineRevision, `${context}.engineRevision`)
  const errorCode = optionalNonEmptyString(record.errorCode, `${context}.errorCode`)
  const message = optionalNonEmptyString(record.message, `${context}.message`)

  return {
    schemaVersion: OFFICE_ENGINE_STATUS_SCHEMA_VERSION,
    providerId,
    availability: expectEnum(
      record.availability,
      ['available', 'unavailable'] as const,
      `${context}.availability`
    ),
    ...(record.source === undefined
      ? {}
      : {
          source: expectEnum(
            record.source,
            ['configured', 'packagedComponent', 'developmentPath'] as const,
            `${context}.source`
          )
        }),
    ...(version === undefined ? {} : { version }),
    ...(engineRevision === undefined ? {} : { engineRevision }),
    capabilities,
    ...(errorCode === undefined ? {} : { errorCode }),
    ...(message === undefined ? {} : { message })
  }
}
