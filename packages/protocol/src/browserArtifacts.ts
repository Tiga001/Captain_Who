import {
  expectEnum,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  expectSchemaVersion,
  expectString,
  hasAsciiControlCharacter,
  invalidProtocolValue
} from './skills/validation'

export const BROWSER_ARTIFACT_SCHEMA_VERSION = 1 as const

export type BrowserArtifactKind =
  | 'image'
  | 'text'
  | 'json'
  | 'pdf'
  | 'trace'
  | 'video'
  | 'download'
  | 'snapshot'
  | 'console'
  | 'network'

export type BrowserArtifactPreview = 'image' | 'text' | 'none'

/**
 * Renderer/model-safe identity for one Host-owned browser Artifact.
 *
 * The opaque id is useful only through the trusted Host API. This projection deliberately omits
 * the managed path, Playwright outputDir, Target identity, tool arguments, and page content.
 */
export interface BrowserArtifactReference {
  schemaVersion: typeof BROWSER_ARTIFACT_SCHEMA_VERSION
  artifactId: string
  kind: BrowserArtifactKind
  displayName: string
  mimeType: string
  sizeBytes: number
  createdAt: number
  expiresAt: number
  lifecycle: 'run'
  owner: 'browser_automation'
  preview: BrowserArtifactPreview
}

export interface BrowserArtifactReadInput {
  schemaVersion: typeof BROWSER_ARTIFACT_SCHEMA_VERSION
  artifact: BrowserArtifactReference
}

export interface BrowserArtifactReadOutput {
  schemaVersion: typeof BROWSER_ARTIFACT_SCHEMA_VERSION
  artifact: BrowserArtifactReference
  bytes: Uint8Array
}

export interface BrowserArtifactToolProjection {
  schemaVersion: typeof BROWSER_ARTIFACT_SCHEMA_VERSION
  type: 'builtin_capability_tool'
  contentOmitted: true
  status: 'completed' | 'failed' | 'cancelled' | 'outcome_unknown'
  artifacts?: BrowserArtifactReference[]
}

const ARTIFACT_ID_PATTERN =
  /^browser-artifact:[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u
const MIME_TYPE_PATTERN = /^[a-z0-9][a-z0-9!#$&^_.+-]{0,63}\/[a-z0-9][a-z0-9!#$&^_.+-]{0,127}$/u
const MAX_DISPLAY_NAME_LENGTH = 128
const MAX_ARTIFACT_BYTES = 128 * 1024 * 1024
const MAX_ARTIFACT_LIFETIME_MS = 24 * 60 * 60 * 1_000
const MAX_IMAGE_PREVIEW_BYTES = 8 * 1024 * 1024
const MAX_TEXT_PREVIEW_BYTES = 256 * 1024

const BROWSER_ARTIFACT_KINDS = [
  'image',
  'text',
  'json',
  'pdf',
  'trace',
  'video',
  'download',
  'snapshot',
  'console',
  'network'
] as const satisfies readonly BrowserArtifactKind[]

export function parseBrowserArtifactReference(
  value: unknown,
  context = 'Browser Artifact reference'
): BrowserArtifactReference {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'artifactId',
      'kind',
      'displayName',
      'mimeType',
      'sizeBytes',
      'createdAt',
      'expiresAt',
      'lifecycle',
      'owner',
      'preview'
    ] as const,
    context
  )
  expectSchemaVersion(record, BROWSER_ARTIFACT_SCHEMA_VERSION, context)
  const artifactId = expectString(record.artifactId, `${context}.artifactId`)
  if (!ARTIFACT_ID_PATTERN.test(artifactId)) {
    throw invalidProtocolValue(`${context}.artifactId`, 'must be an opaque Browser Artifact id')
  }
  const displayName = expectString(record.displayName, `${context}.displayName`)
  const canonicalDisplayName = displayName.normalize('NFKC').trim()
  if (
    displayName !== canonicalDisplayName ||
    displayName.length < 1 ||
    displayName.length > MAX_DISPLAY_NAME_LENGTH ||
    hasAsciiControlCharacter(displayName) ||
    displayName === '.' ||
    displayName === '..' ||
    displayName.startsWith('.') ||
    displayName.endsWith('.') ||
    /[<>:"/\\|?*]/u.test(displayName)
  ) {
    throw invalidProtocolValue(
      `${context}.displayName`,
      'must be a canonical bounded path-free name'
    )
  }
  const rawMimeType = expectString(record.mimeType, `${context}.mimeType`)
  const mimeType = rawMimeType.toLowerCase()
  if (rawMimeType !== mimeType || !MIME_TYPE_PATTERN.test(mimeType)) {
    throw invalidProtocolValue(`${context}.mimeType`, 'must be a canonical MIME type')
  }
  const sizeBytes = expectSafeInteger(record.sizeBytes, `${context}.sizeBytes`, 0)
  if (sizeBytes > MAX_ARTIFACT_BYTES) {
    throw invalidProtocolValue(`${context}.sizeBytes`, 'exceeds the protocol byte limit')
  }
  const createdAt = expectSafeInteger(record.createdAt, `${context}.createdAt`, 0)
  const expiresAt = expectSafeInteger(record.expiresAt, `${context}.expiresAt`, 1)
  if (expiresAt <= createdAt || expiresAt - createdAt > MAX_ARTIFACT_LIFETIME_MS) {
    throw invalidProtocolValue(`${context}.expiresAt`, 'must be a bounded future expiry')
  }
  if (record.lifecycle !== 'run' || record.owner !== 'browser_automation') {
    throw invalidProtocolValue(context, 'contains an unsupported lifecycle or owner')
  }
  return {
    schemaVersion: BROWSER_ARTIFACT_SCHEMA_VERSION,
    artifactId,
    kind: expectEnum(record.kind, BROWSER_ARTIFACT_KINDS, `${context}.kind`),
    displayName,
    mimeType,
    sizeBytes,
    createdAt,
    expiresAt,
    lifecycle: 'run',
    owner: 'browser_automation',
    preview: expectEnum(record.preview, ['image', 'text', 'none'] as const, `${context}.preview`)
  }
}

export function parseBrowserArtifactReadInput(value: unknown): BrowserArtifactReadInput {
  const context = 'Browser Artifact read request'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'artifact'] as const, context)
  expectSchemaVersion(record, BROWSER_ARTIFACT_SCHEMA_VERSION, context)
  return {
    schemaVersion: BROWSER_ARTIFACT_SCHEMA_VERSION,
    artifact: parseBrowserArtifactReference(record.artifact, `${context}.artifact`)
  }
}

export function parseBrowserArtifactReadOutput(value: unknown): BrowserArtifactReadOutput {
  const context = 'Browser Artifact read response'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'artifact', 'bytes'] as const, context)
  expectSchemaVersion(record, BROWSER_ARTIFACT_SCHEMA_VERSION, context)
  const artifact = parseBrowserArtifactReference(record.artifact, `${context}.artifact`)
  if (artifact.preview === 'none') {
    throw invalidProtocolValue(`${context}.artifact.preview`, 'does not permit Renderer content')
  }
  const bytes = record.bytes
  if (!(bytes instanceof Uint8Array) || bytes.byteLength !== artifact.sizeBytes) {
    throw invalidProtocolValue(`${context}.bytes`, 'must match the declared Artifact byte length')
  }
  const maximumBytes =
    artifact.preview === 'image' ? MAX_IMAGE_PREVIEW_BYTES : MAX_TEXT_PREVIEW_BYTES
  if (bytes.byteLength > maximumBytes) {
    throw invalidProtocolValue(`${context}.bytes`, 'exceeds the bounded preview byte limit')
  }
  return {
    schemaVersion: BROWSER_ARTIFACT_SCHEMA_VERSION,
    artifact,
    bytes: Uint8Array.from(bytes)
  }
}

export function parseBrowserArtifactToolProjection(value: unknown): BrowserArtifactToolProjection {
  const context = 'Browser Artifact tool projection'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['schemaVersion', 'type', 'contentOmitted', 'status', 'artifacts'] as const,
    context
  )
  expectSchemaVersion(record, BROWSER_ARTIFACT_SCHEMA_VERSION, context)
  if (record.type !== 'builtin_capability_tool' || record.contentOmitted !== true) {
    throw invalidProtocolValue(context, 'must be a safe builtin capability projection')
  }
  let artifacts: BrowserArtifactReference[] | undefined
  if (record.artifacts !== undefined) {
    if (
      !Array.isArray(record.artifacts) ||
      record.artifacts.length < 1 ||
      record.artifacts.length > 16
    ) {
      throw invalidProtocolValue(`${context}.artifacts`, 'must contain one to sixteen references')
    }
    artifacts = record.artifacts.map((artifact, index) =>
      parseBrowserArtifactReference(artifact, `${context}.artifacts[${index}]`)
    )
    if (new Set(artifacts.map((artifact) => artifact.artifactId)).size !== artifacts.length) {
      throw invalidProtocolValue(`${context}.artifacts`, 'contains duplicate Artifact ids')
    }
  }
  return {
    schemaVersion: BROWSER_ARTIFACT_SCHEMA_VERSION,
    type: 'builtin_capability_tool',
    contentOmitted: true,
    status: expectEnum(
      record.status,
      ['completed', 'failed', 'cancelled', 'outcome_unknown'] as const,
      `${context}.status`
    ),
    ...(artifacts ? { artifacts } : {})
  }
}
