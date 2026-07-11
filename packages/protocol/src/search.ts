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
