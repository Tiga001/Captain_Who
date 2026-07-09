import type { ChatSearchInput, ChatSearchResult } from '@mycopilot/protocol'
import { hostClient } from '../../host/hostClient'

export type { ChatSearchInput, ChatSearchResult }

export async function searchChats(input: ChatSearchInput): Promise<ChatSearchResult[]> {
  const searchHost = (
    hostClient as typeof hostClient & {
      search?: {
        searchChats?: (input: ChatSearchInput) => Promise<ChatSearchResult[]>
      }
    }
  ).search

  if (!searchHost?.searchChats) {
    throw new Error('Search host API is not available. Restart the Electron app to reload preload.')
  }

  return searchHost.searchChats(input)
}
