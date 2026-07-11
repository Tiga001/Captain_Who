import { Globe2, TerminalSquare } from 'lucide-react'
import type { RightSidebarModuleDefinition, RightSidebarModuleId } from './rightSidebarTypes'

export const RIGHT_SIDEBAR_MODULES: RightSidebarModuleDefinition[] = [
  {
    id: 'terminal',
    titleKey: 'rightSidebar.terminal',
    icon: TerminalSquare
  },
  {
    id: 'browser',
    titleKey: 'rightSidebar.browser',
    icon: Globe2
  }
]

export function getRightSidebarModule(moduleId: RightSidebarModuleId) {
  return RIGHT_SIDEBAR_MODULES.find((module) => module.id === moduleId) ?? null
}
