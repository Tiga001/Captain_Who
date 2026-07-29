// Pure sidebar helpers for ordering, labels, and menu placement.
import type { AppProject } from '../../../config/projectConfig'
import type { AppLanguage } from '../../../config/frontendTranslations'
import type { ChatConversation } from '../../../features/chat/chatTypes'
import type {
  SidebarConversationSort,
  SidebarProjectSort
} from '../../../features/storage/storageClient'
import type { SidebarMenuPosition } from './leftSidebarTypes'

export const COLLAPSED_CONVERSATION_COUNT = 5
export const PREVIEW_CONVERSATION_COUNT = 10
export const PREVIEW_CONVERSATION_THRESHOLD = 12
export const ROOT_CONVERSATION_LIST_KEY = 'root'
const SIDEBAR_MENU_MARGIN = 12
const SIDEBAR_MENU_WIDTH = 224
export const SIDEBAR_SECTION_MENU_ESTIMATED_HEIGHT = 172
export const SIDEBAR_PROJECT_MENU_ESTIMATED_HEIGHT = 224

type ConversationAgeUnit = 'day' | 'hour' | 'minute' | 'month' | 'week' | 'year'
type ConversationAgeFormatterKey = `${AppLanguage}:${ConversationAgeUnit}`

const conversationAgeFormatters = new Map<ConversationAgeFormatterKey, Intl.NumberFormat>()

function formatConversationAgeUnit(
  value: number,
  unit: ConversationAgeUnit,
  language: AppLanguage
) {
  const formatterKey: ConversationAgeFormatterKey = `${language}:${unit}`
  let formatter = conversationAgeFormatters.get(formatterKey)

  if (!formatter) {
    formatter = new Intl.NumberFormat(language, {
      style: 'unit',
      unit,
      unitDisplay: 'narrow'
    })
    conversationAgeFormatters.set(formatterKey, formatter)
  }

  return formatter.format(value)
}

export function formatTemplate(template: string, values: Record<string, number | string>) {
  return Object.entries(values).reduce(
    (text, [key, value]) => text.split(`{${key}}`).join(String(value)),
    template
  )
}

export function formatConversationAge(
  updatedAt: number,
  now: number,
  language: AppLanguage,
  justNow: string
) {
  const elapsed = Math.max(0, now - updatedAt)
  const minutes = Math.floor(elapsed / 60_000)
  if (minutes < 1) return justNow

  if (minutes < 60) {
    return formatConversationAgeUnit(minutes, 'minute', language)
  }

  const hours = Math.floor(elapsed / 3_600_000)
  if (hours < 24) {
    return formatConversationAgeUnit(hours, 'hour', language)
  }

  const days = Math.floor(elapsed / 86_400_000)
  if (days < 7) {
    return formatConversationAgeUnit(days, 'day', language)
  }

  const weeks = Math.floor(days / 7)
  if (weeks < 5) {
    return formatConversationAgeUnit(weeks, 'week', language)
  }

  const months = Math.floor(days / 30)
  if (months < 12) {
    return formatConversationAgeUnit(months, 'month', language)
  }

  const years = Math.floor(days / 365)
  return formatConversationAgeUnit(Math.max(1, years), 'year', language)
}

export function sortConversations(
  conversations: ChatConversation[],
  sort: SidebarConversationSort
) {
  return [...conversations].sort((a, b) => {
    const aPinned = a.pinnedAt ?? 0
    const bPinned = b.pinnedAt ?? 0

    if (aPinned || bPinned) {
      if (aPinned && bPinned) return bPinned - aPinned
      return aPinned ? -1 : 1
    }

    if (sort === 'created') return b.createdAt - a.createdAt
    return b.updatedAt - a.updatedAt
  })
}

export function clampMenuPosition(
  left: number,
  top: number,
  estimatedHeight: number
): SidebarMenuPosition {
  return {
    left: Math.max(
      SIDEBAR_MENU_MARGIN,
      Math.min(left, window.innerWidth - SIDEBAR_MENU_WIDTH - SIDEBAR_MENU_MARGIN)
    ),
    top: Math.max(
      SIDEBAR_MENU_MARGIN,
      Math.min(top, window.innerHeight - estimatedHeight - SIDEBAR_MENU_MARGIN)
    )
  }
}

export function sortPinnedProjects(projects: AppProject[]) {
  return [...projects]
    .filter((project) => project.pinnedAt)
    .sort((a, b) => (b.pinnedAt ?? 0) - (a.pinnedAt ?? 0))
}

export function sortProjectsByManualOrder(projects: AppProject[], projectOrder: string[]) {
  const orderIndex = new Map(projectOrder.map((projectId, index) => [projectId, index]))

  return [...projects].sort((a, b) => {
    const aIndex = orderIndex.get(a.id)
    const bIndex = orderIndex.get(b.id)

    if (aIndex !== undefined || bIndex !== undefined) {
      if (aIndex !== undefined && bIndex !== undefined) return aIndex - bIndex
      return aIndex !== undefined ? -1 : 1
    }

    return a.createdAt - b.createdAt
  })
}

export function sortRegularProjects(
  projects: AppProject[],
  conversationsByProjectId: Record<string, ChatConversation[]>,
  sort: SidebarProjectSort,
  projectOrder: string[]
) {
  const regularProjects = projects.filter((project) => !project.pinnedAt)

  if (sort === 'manual') {
    return sortProjectsByManualOrder(regularProjects, projectOrder)
  }

  return [...regularProjects].sort((a, b) => {
    if (sort === 'recent') {
      const aRecent = latestProjectConversationUpdatedAt(conversationsByProjectId[a.id] ?? [])
      const bRecent = latestProjectConversationUpdatedAt(conversationsByProjectId[b.id] ?? [])
      if (aRecent !== bRecent) return bRecent - aRecent
    }

    return a.createdAt - b.createdAt
  })
}

function latestProjectConversationUpdatedAt(conversations: ChatConversation[]) {
  return conversations.reduce((latest, conversation) => Math.max(latest, conversation.updatedAt), 0)
}
