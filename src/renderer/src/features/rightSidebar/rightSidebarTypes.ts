import type { LucideIcon } from 'lucide-react'
import type { ReactNode } from 'react'
import type { TranslationKey } from '../../config/frontendTranslations'
import type { Translate } from '../../config/translationFormat'
import type { AgentSummary, GitReviewTarget } from '@mycopilot/protocol'

export type RightSidebarModuleId = 'terminal' | 'browser' | 'files' | 'git-review' | 'agent-center'

export type RightSidebarSurfaceKind = 'react' | 'webview'

export type RightSidebarRetentionPolicy = 'keep-alive' | 'unmount-when-inactive'

export type RightSidebarInstancePolicy = 'multiple' | 'single' | 'single-per-workspace'

export type RightSidebarContextBinding =
  'global' | 'pinned-to-creation-workspace' | 'follow-workspace'

export type RightSidebarCapabilityId = 'git-repository'

export type RightSidebarCapabilityStatus = 'checking' | 'available' | 'unavailable'

export interface RightSidebarCapabilityState {
  contextKey: string
  identity?: string
  status: RightSidebarCapabilityStatus
}

export type RightSidebarCapabilities = Partial<
  Record<RightSidebarCapabilityId, RightSidebarCapabilityState>
>

export type RightSidebarModuleAvailability = RightSidebarCapabilityStatus

export type RightSidebarModuleAvailabilityMap = Partial<
  Record<RightSidebarModuleId, RightSidebarModuleAvailability>
>

export type RightSidebarUnavailablePagePolicy = 'close-page' | 'retain-page'

export type RightSidebarOrphanedWorkspacePolicy = 'close-page' | 'retain-page'

export type RightSidebarActivity = 'foreground' | 'background' | 'dormant'

export interface RightSidebarWorkspaceContext {
  hasWorkspace: boolean
  key: string
  name: string | null
  path?: string
  sessionKey: string
}

export interface RightSidebarModuleCreateContext {
  existingPages: RightSidebarPage[]
  pageId: string
  t: Translate
  workspace: RightSidebarWorkspaceContext
}

export interface RightSidebarPageUpdate {
  iconUrl?: string | null
  moduleState?: RightSidebarModulePageState
  title?: string
}

export type RightSidebarModulePageState =
  | {
      kind: 'browser-surface'
      surfaceId: string
      url?: string
      viewport?: { height: number; width: number }
    }
  | {
      kind: 'workspace-file'
      path: string
      folderId?: string
      assistantMessageId?: string
      tabState?: 'stable' | 'transient'
      preview?: {
        markdownAnchor?: string
        markdownView?: 'preview' | 'source'
        pdfPage?: number
        wrapLines?: boolean
      }
    }
  | {
      kind: 'git-review'
      filePath?: string
      projectId: string
      requestId: number
      target: Extract<GitReviewTarget, { kind: 'lastTurn' }>
    }
  | {
      agentId?: string
      kind: 'agent-center'
      rootConversationId: string
      view: 'list' | 'detail'
    }

export type RightSidebarReviewNavigationRequest = Extract<
  RightSidebarModulePageState,
  { kind: 'git-review' }
>

export interface RightSidebarPageOpenRequest {
  disposition?: 'new-page' | 'preview' | 'reuse-source-if-empty'
  iconUrl?: string | null
  moduleState?: RightSidebarModulePageState
  resourceKey?: string
  targetModuleId?: RightSidebarModuleId
  title: string
}

export interface RightSidebarModuleRenderProps {
  activity: RightSidebarActivity
  availability: RightSidebarModuleAvailability
  isSelected: boolean
  onOpenPage: (request: RightSidebarPageOpenRequest) => void
  onPageUpdate: (update: RightSidebarPageUpdate) => void
  onSurfaceFocus: () => void
  page: RightSidebarPage
  t: Translate
}

export interface RightSidebarModuleDefinition {
  badge?: number
  contextBinding: RightSidebarContextBinding
  createPage: (context: RightSidebarModuleCreateContext) => RightSidebarPage
  id: RightSidebarModuleId
  icon: LucideIcon
  instancePolicy: RightSidebarInstancePolicy
  maxRelatedPagesPerWorkspace?: number
  orphanedWorkspacePolicy?: RightSidebarOrphanedWorkspacePolicy
  render: (props: RightSidebarModuleRenderProps) => ReactNode
  requiredCapability?: RightSidebarCapabilityId
  requiresWorkspace?: boolean
  retention: RightSidebarRetentionPolicy
  surfaceKind: RightSidebarSurfaceKind
  titleKey: TranslationKey
  unavailablePagePolicy: RightSidebarUnavailablePagePolicy
}

export interface AgentObserverRenderContext {
  agent: AgentSummary
  agentLabelsById: Readonly<Record<string, string>>
  invalidationVersion: string
  rootConversationId: string
}

export interface RightSidebarAgentNavigationRequest {
  agentId: string
  requestId: number
  rootConversationId: string
}

export interface RightSidebarModuleNavigationRequest {
  conversationId?: string | null
  moduleId: RightSidebarModuleId
  requestId: number
  workspaceKey?: string | null
  workspacePath?: string
}

export interface RightSidebarPage {
  iconUrl?: string | null
  id: string
  moduleState?: RightSidebarModulePageState
  moduleId: RightSidebarModuleId
  resourceKey?: string
  title: string
  workspaceKey?: string | null
  workspaceName?: string | null
  workspacePath?: string
  workspaceSessionKey?: string | null
}
