export const SEARCH_SEARCH_CHATS_METHOD = 'search.searchChats'

export type ChatSearchMatchKind = 'title' | 'message'

export interface ChatSearchInput {
  query: string
  limit?: number
}

export interface ChatSearchResult {
  conversationId: string
  projectId?: string | null
  title: string
  messageId?: string | null
  snippet?: string | null
  matchKind: ChatSearchMatchKind
  updatedAt: number
}
