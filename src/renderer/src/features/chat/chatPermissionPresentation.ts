import { ShieldAlert, ShieldCheck, ShieldPlus, type LucideIcon } from 'lucide-react'
import type { TranslationKey } from '../../config/frontendTranslations'
import type { ChatPermissionMode } from './chatTypes'

export interface ChatPermissionPresentation {
  icon: LucideIcon
  id: ChatPermissionMode
  labelKey: TranslationKey
}

/** Shared visual and copy contract for every Renderer permission-mode selector. */
export const CHAT_PERMISSION_PRESENTATIONS: readonly ChatPermissionPresentation[] = [
  { id: 'default', labelKey: 'chat.defaultPermission', icon: ShieldPlus },
  { id: 'full', labelKey: 'chat.fullPermission', icon: ShieldAlert },
  { id: 'custom', labelKey: 'chat.customPermission', icon: ShieldCheck }
]

export function getChatPermissionPresentation(
  mode: ChatPermissionMode
): ChatPermissionPresentation {
  return (
    CHAT_PERMISSION_PRESENTATIONS.find((presentation) => presentation.id === mode) ??
    CHAT_PERMISSION_PRESENTATIONS[0]
  )
}
