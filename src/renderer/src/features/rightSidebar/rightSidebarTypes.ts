import type { LucideIcon } from 'lucide-react'
import type { TranslationKey } from '../../config/frontendTranslations'

export type RightSidebarModuleId = 'terminal' | 'browser'

export interface RightSidebarModuleDefinition {
  id: RightSidebarModuleId
  titleKey: TranslationKey
  icon: LucideIcon
}

export interface RightSidebarPage {
  iconUrl?: string | null
  id: string
  moduleId: RightSidebarModuleId
  title: string
  workspaceKey?: string | null
  workspacePath?: string
}
