import type { AgentFileInputBinding, AgentFileInputSpec } from './command'
import type { AgentApprovalStatus } from './core'

export type OfficeDocumentKind = 'document' | 'spreadsheet' | 'presentation'

export type OfficeOperation =
  | 'help'
  | 'create'
  | 'view'
  | 'get'
  | 'query'
  | 'validate'
  | 'set'
  | 'add'
  | 'remove'
  | 'move'
  | 'swap'

/** Describes whether the normalized operation has file side effects.
 *
 * Path location is expressed independently by each OfficeFrozenPath.scope.
 */
export type OfficeOperationAccess = 'readOnly' | 'fileWrite'

/** Provider-neutral help topic.
 *
 * status/help/create/view/validate/move/swap are served by the Host. The remaining topics select
 * provider element-schema help and may be paired with an element name.
 */
export type OfficeHelpVerb =
  | 'status'
  | 'help'
  | 'create'
  | 'view'
  | 'get'
  | 'query'
  | 'validate'
  | 'set'
  | 'add'
  | 'remove'
  | 'move'
  | 'swap'

export type OfficeViewMode =
  'text' | 'annotated' | 'outline' | 'stats' | 'issues' | 'html' | 'svg' | 'screenshot' | 'forms'

export type OfficeViewRenderMode = 'auto' | 'html'

export type OfficeGridLayout = { mode: 'auto' } | { mode: 'columns'; columns: number }

export type OfficeCellShift = 'left' | 'up'

export type OfficeElementPosition =
  | { type: 'index'; index: number }
  | { type: 'after'; target: string }
  | { type: 'before'; target: string }

export interface OfficeTextReplacement {
  find: string
  replace: string
}

export interface OfficePageRange {
  start: number
  end?: number
}

export interface OfficeViewport {
  width: number
  height: number
}

export interface OfficePresentationRenderPlan {
  requestedPages: number[]
  slideWidthEmu: number
  slideHeightEmu: number
  viewport: OfficeViewport
  grid: OfficeGridLayout | null
}

export interface OfficeRenderGridGeometry {
  columns: number
  rows: number
  viewportWidth: number
  viewportHeight: number
  contentWidth: number
  contentHeight: number
}

/**
 * Host-verified renderer geometry only. This proves that the frozen requested
 * slide set fits the decoded PNG viewport under the trusted layout formula; it
 * is not visual-content evidence and does not replace per-slide inspection.
 */
export interface OfficeRenderLayoutCoverage {
  requestedPages: number[]
  evidence: 'trustedRendererGeometry'
  grid?: OfficeRenderGridGeometry
}

export type OfficePropertyValue = string | number | boolean | { resourcePath: string }

export type OfficePropertyMap = Record<string, OfficePropertyValue>

export type OfficeOperationParameters =
  | { type: 'help'; verb?: OfficeHelpVerb; element?: string }
  | { type: 'create'; locale?: string; minimal?: boolean; overwrite?: boolean }
  | {
      type: 'view'
      mode: OfficeViewMode
      start?: number
      end?: number
      maxLines?: number
      issueType?: string
      limit?: number
      columns?: string[]
      pages?: OfficePageRange[]
      range?: string
      viewport?: OfficeViewport
      grid?: OfficeGridLayout
      renderMode?: OfficeViewRenderMode
      pageCount?: boolean
    }
  | { type: 'get'; target?: string; depth?: number }
  | { type: 'query'; selector: string; contains?: string; compact?: boolean; fields?: string[] }
  | { type: 'validate' }
  | {
      type: 'set'
      target: string
      properties?: OfficePropertyMap
      replacement?: OfficeTextReplacement
      force?: boolean
    }
  | {
      type: 'add'
      parent: string
      elementType: string
      copyFrom?: string
      position?: OfficeElementPosition
      properties?: OfficePropertyMap
      force?: boolean
    }
  | { type: 'remove'; target: string; shift?: OfficeCellShift; properties?: OfficePropertyMap }
  | {
      type: 'move'
      target: string
      newParent?: string
      position?: OfficeElementPosition
      properties?: OfficePropertyMap
    }
  | { type: 'swap'; firstTarget: string; secondTarget: string }

interface OfficeExecutionRequestBase {
  documentKind: OfficeDocumentKind
  operation: OfficeOperation
  documentPath: string | null
  outputPath: string | null
  destinationPath: string | null
  inputs: AgentFileInputSpec[]
  timeoutMs: number | null
}

export type OfficeExecutionRequest = OfficeExecutionRequestBase & {
  parameters: OfficeOperationParameters
}

export type OfficeFilePreconditionState = 'missing' | 'present'

export type OfficePathSlot =
  | { type: 'document' }
  | { type: 'output' }
  | { type: 'destination' }
  | { type: 'resource'; index: number }

export type OfficePathPurpose = 'readSource' | 'writeTarget' | 'inPlaceTarget'

export type OfficePathScope = 'workspace' | 'external' | 'attachment'

export type OfficeWriteDisposition = 'createNew' | 'replaceExisting'

export interface OfficePathIdentity {
  revision: string
  device: number | null
  inode: number | null
}

/**
 * One backend-authorized path in an immutable Office execution snapshot.
 * The Renderer may display this metadata but must never derive authorization
 * or approval requirements from it.
 */
export interface OfficeFrozenPath {
  slot: OfficePathSlot
  logicalPath: string
  purpose: OfficePathPurpose
  scope: OfficePathScope
  normalizedPath: string
  state: OfficeFilePreconditionState
  objectIdentity: OfficePathIdentity | null
  parentIdentity: OfficePathIdentity
  contentRevision: string | null
  size: number | null
  writeDisposition: OfficeWriteDisposition | null
}

/** Immutable, shell-free Office execution snapshot prepared by the trusted backend. */
export interface OfficePreparedExecution {
  schemaVersion: number
  providerId: string
  engineRevision: string
  workspaceRevision: string | null
  access: OfficeOperationAccess
  request: OfficeExecutionRequest
  argv: string[]
  /** Host-owned deterministic presentation render plan; null for other operations. */
  resolvedRenderPlan: OfficePresentationRenderPlan | null
  paths: OfficeFrozenPath[]
  inputBindings: AgentFileInputBinding[]
}

export interface AgentOfficeOperationRequest {
  schemaVersion: number
  id: string
  /** Exact, model-facing Office Tool arguments frozen by the Host. */
  semanticArgs: Record<string, unknown>
  prepared: OfficePreparedExecution
  approvalStatus: AgentApprovalStatus
  reason: string
}
