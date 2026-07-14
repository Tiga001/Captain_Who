import type { LucideIcon } from 'lucide-react'
import type { ReactNode } from 'react'
import type { TranslationKey } from '../../config/frontendTranslations'
import type { Translate } from '../../config/translationFormat'

export type RightSidebarModuleId = 'terminal' | 'browser'

export type RightSidebarSurfaceKind = 'react' | 'webview'

export type RightSidebarRetentionPolicy = 'keep-alive' | 'unmount-when-inactive'

export interface RightSidebarWorkspaceContext {
  key: string
  name: string | null
  path?: string
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

export interface RightSidebarModuleRenderProps {
  isActive: boolean
  onPageUpdate: (update: RightSidebarPageUpdate) => void
  onSurfaceFocus: () => void
  page: RightSidebarPage
  t: Translate
}

export interface RightSidebarModuleDefinition {
  createPage: (context: RightSidebarModuleCreateContext) => RightSidebarPage
  id: RightSidebarModuleId
  icon: LucideIcon
  render: (props: RightSidebarModuleRenderProps) => ReactNode
  retention: RightSidebarRetentionPolicy
  surfaceKind: RightSidebarSurfaceKind
  titleKey: TranslationKey
}

export interface RightSidebarPage {
  iconUrl?: string | null
  id: string
  moduleId: RightSidebarModuleId
  title: string
  workspaceKey?: string | null
  workspacePath?: string
}
