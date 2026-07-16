import type { LucideIcon } from 'lucide-react'
import type { ReactNode } from 'react'
import type { TranslationKey } from '../../config/frontendTranslations'
import type { Translate } from '../../config/translationFormat'

export type RightSidebarModuleId = 'terminal' | 'browser' | 'files' | 'git-review'

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
  title?: string
}

export type RightSidebarModulePageState = {
  kind: 'workspace-file'
  path: string
}

export interface RightSidebarPageOpenRequest {
  iconUrl?: string | null
  moduleState?: RightSidebarModulePageState
  resourceKey?: string
  title: string
}

export interface RightSidebarModuleRenderProps {
  availability: RightSidebarModuleAvailability
  isActive: boolean
  onOpenPage: (request: RightSidebarPageOpenRequest) => void
  onPageUpdate: (update: RightSidebarPageUpdate) => void
  onSurfaceFocus: () => void
  page: RightSidebarPage
  t: Translate
}

export interface RightSidebarModuleDefinition {
  contextBinding: RightSidebarContextBinding
  createPage: (context: RightSidebarModuleCreateContext) => RightSidebarPage
  id: RightSidebarModuleId
  icon: LucideIcon
  instancePolicy: RightSidebarInstancePolicy
  render: (props: RightSidebarModuleRenderProps) => ReactNode
  requiredCapability?: RightSidebarCapabilityId
  requiresWorkspace?: boolean
  retention: RightSidebarRetentionPolicy
  surfaceKind: RightSidebarSurfaceKind
  titleKey: TranslationKey
  unavailablePagePolicy: RightSidebarUnavailablePagePolicy
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
