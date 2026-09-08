export type ActiveRunBinding = {
  conversationId: string
  pendingMessageId: string
}

export type AutoSubmitQueuedMessage = (conversationId: string, action?: 'pause') => void
