import type { AgentPermissions, AgentPromptPreferences } from '@mycopilot/protocol'
import type { StorageUiPreferencesRecord } from '@mycopilot/protocol'

export interface AgentPromptPreferencesSnapshot extends AgentPromptPreferences {
  contextProfile: 'full' | 'minimal'
  workMode: 'coding' | 'general'
  tone: 'friendly' | 'pragmatic'
  detailLevel: 'low' | 'medium' | 'high'
  customInstructions: string
  updatedAt: number
}

export type SidebarConversationSort = 'created' | 'updated'
export type SidebarProjectSort = 'created' | 'recent' | 'manual'
type SidebarSectionOrder = 'projects_first' | 'conversations_first'

export const MIN_TRANSLUCENT_SIDEBAR_TRANSPARENCY = 50
export const MAX_TRANSLUCENT_SIDEBAR_TRANSPARENCY = 100
const DEFAULT_TRANSLUCENT_SIDEBAR_TRANSPARENCY = 54
const TRANSLUCENT_SIDEBAR_THEME_TINT_FLOOR = 32

export interface UiPreferencesSnapshot {
  profileAvatarDataUrl: string | null
  profileDisplayName: string
  profileHandle: string
  sidebarConversationSort: SidebarConversationSort
  sidebarProjectSort: SidebarProjectSort
  sidebarProjectOrder: string[]
  sidebarSectionOrder: SidebarSectionOrder
  nativeFontSmoothing: boolean
  showTokenUsageDetails: boolean
  showContextWindowUsage: boolean
  translucentSidebar: boolean
  translucentSidebarTransparency: number
  fullPermissionEnabled: boolean
  customPermissionEnabled: boolean
  customPermissions: AgentPermissions
  updatedAt: number
}

export function defaultAgentPromptPreferences(): AgentPromptPreferencesSnapshot {
  return {
    contextProfile: 'full',
    workMode: 'coding',
    tone: 'pragmatic',
    detailLevel: 'medium',
    customInstructions: '',
    updatedAt: 0
  }
}

export function defaultUiPreferences(): UiPreferencesSnapshot {
  return {
    profileAvatarDataUrl: null,
    profileDisplayName: '',
    profileHandle: 'USER',
    sidebarConversationSort: 'updated',
    sidebarProjectSort: 'created',
    sidebarProjectOrder: [],
    sidebarSectionOrder: 'projects_first',
    nativeFontSmoothing: false,
    showTokenUsageDetails: true,
    showContextWindowUsage: true,
    translucentSidebar: false,
    translucentSidebarTransparency: DEFAULT_TRANSLUCENT_SIDEBAR_TRANSPARENCY,
    fullPermissionEnabled: true,
    customPermissionEnabled: true,
    customPermissions: {
      read: 'workspace_only',
      write: 'workspace_only',
      command: 'require_approval',
      commandSafety: 'guarded',
      patch: 'require_approval',
      builtinExecution: 'require_approval'
    },
    updatedAt: 0
  }
}

export function normalizeAgentPromptPreferences(
  preferences: AgentPromptPreferences | null | undefined
): AgentPromptPreferencesSnapshot {
  const defaults = defaultAgentPromptPreferences()
  return {
    contextProfile: preferences?.contextProfile === 'minimal' ? 'minimal' : 'full',
    workMode: preferences?.workMode === 'general' ? 'general' : defaults.workMode,
    tone: preferences?.tone === 'friendly' ? 'friendly' : defaults.tone,
    detailLevel:
      preferences?.detailLevel === 'low' || preferences?.detailLevel === 'high'
        ? preferences.detailLevel
        : defaults.detailLevel,
    customInstructions:
      typeof preferences?.customInstructions === 'string'
        ? preferences.customInstructions.trim()
        : '',
    updatedAt: typeof preferences?.updatedAt === 'number' ? preferences.updatedAt : Date.now()
  }
}

export function normalizeUiPreferences(
  preferences: StorageUiPreferencesRecord | null | undefined
): UiPreferencesSnapshot {
  return {
    ...defaultUiPreferences(),
    ...preferences,
    profileAvatarDataUrl: preferences?.profileAvatarDataUrl ?? null,
    sidebarConversationSort:
      preferences?.sidebarConversationSort === 'created' ? 'created' : 'updated',
    sidebarProjectSort:
      preferences?.sidebarProjectSort === 'recent' || preferences?.sidebarProjectSort === 'manual'
        ? preferences.sidebarProjectSort
        : 'created',
    sidebarProjectOrder: Array.isArray(preferences?.sidebarProjectOrder)
      ? preferences.sidebarProjectOrder
      : [],
    sidebarSectionOrder:
      preferences?.sidebarSectionOrder === 'conversations_first'
        ? 'conversations_first'
        : 'projects_first',
    showContextWindowUsage: preferences?.showContextWindowUsage !== false,
    translucentSidebarTransparency: normalizeTranslucentSidebarTransparency(
      preferences?.translucentSidebarTransparency
    ),
    customPermissions: {
      ...defaultUiPreferences().customPermissions,
      ...preferences?.customPermissions,
      commandSafety: 'guarded'
    },
    updatedAt: typeof preferences?.updatedAt === 'number' ? preferences.updatedAt : Date.now()
  }
}

export function normalizeTranslucentSidebarTransparency(value: unknown): number {
  const numericValue =
    typeof value === 'number' && Number.isFinite(value)
      ? Math.round(value)
      : DEFAULT_TRANSLUCENT_SIDEBAR_TRANSPARENCY
  return Math.min(
    MAX_TRANSLUCENT_SIDEBAR_TRANSPARENCY,
    Math.max(MIN_TRANSLUCENT_SIDEBAR_TRANSPARENCY, numericValue)
  )
}

export function getTranslucentSidebarOpacityPercent(transparency: unknown): string {
  const requestedTintOpacity = 100 - normalizeTranslucentSidebarTransparency(transparency)

  // Native macOS vibrancy only knows the system appearance, not the selected Captain Who palette.
  // Keep a perceptual theme floor above it so maximum transparency remains themed glass instead
  // of visually collapsing to the native gray sidebar material.
  const effectiveTintOpacity =
    TRANSLUCENT_SIDEBAR_THEME_TINT_FLOOR +
    requestedTintOpacity * (1 - TRANSLUCENT_SIDEBAR_THEME_TINT_FLOOR / 100)

  return `${Math.round(effectiveTintOpacity)}%`
}
