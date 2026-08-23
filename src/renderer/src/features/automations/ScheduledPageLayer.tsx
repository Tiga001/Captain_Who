import type { ModelConfig } from '../../config/modelConfig'
import type { AppProject } from '../../config/projectConfig'
import type { ChatConversation, ChatPermissionMode } from '../chat/chatTypes'
import {
  ScheduledPage,
  type ScheduledExternalNavigationRequest,
  type ScheduledOpenRequest
} from './ScheduledPage'
import './ScheduledPage.css'

export interface ScheduledPageLayerProps {
  conversations: readonly ChatConversation[]
  defaultModelId: string | null
  defaultPermissionMode: ChatPermissionMode
  defaultProjectId: string | null
  externalNavigationRequest?: ScheduledExternalNavigationRequest
  initialPreferredDrawerWidth?: number
  models: readonly ModelConfig[]
  onOpenConversation: (conversationId: string, messageId?: string | null) => void
  onOpenPermissionSettings?: () => void
  onPreferredDrawerWidthChange?: (width: number) => void
  openRequest?: ScheduledOpenRequest
  permissionModeAvailability: { custom: boolean; full: boolean }
  projects: readonly AppProject[]
}

export function ScheduledPageLayer(props: ScheduledPageLayerProps) {
  return (
    <div className="scheduled-page-layer" data-testid="scheduled-page-layer">
      <ScheduledPage {...props} />
    </div>
  )
}
