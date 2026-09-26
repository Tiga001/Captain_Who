import { createContext, useContext, type ReactNode } from 'react'

const ConversationNavigationContext = createContext<((conversationId: string) => void) | null>(null)

export function ConversationNavigationProvider({
  children,
  onOpenConversation
}: {
  children: ReactNode
  onOpenConversation: (conversationId: string) => void
}) {
  return (
    <ConversationNavigationContext.Provider value={onOpenConversation}>
      {children}
    </ConversationNavigationContext.Provider>
  )
}

export function useConversationNavigation() {
  return useContext(ConversationNavigationContext)
}
