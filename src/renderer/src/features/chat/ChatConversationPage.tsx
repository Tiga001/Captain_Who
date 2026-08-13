import type { ComponentProps } from 'react'
import {
  ConversationSurface,
  type InteractiveConversationSurfaceProps
} from './ConversationSurface'

export {
  ChatMessageList,
  ConversationContinuationDivider,
  ConversationSurface
} from './ConversationSurface'
export type {
  ConversationSurfaceProps,
  InteractiveConversationSurfaceProps,
  ObserverConversationSurfaceProps
} from './ConversationSurface'

/**
 * Root-conversation adapter kept for the existing AppShell route. Shared Timeline and scrolling
 * behavior live in ConversationSurface; this adapter can only select the interactive capability
 * branch.
 */
export type ChatConversationPageProps = Omit<InteractiveConversationSurfaceProps, 'mode'>

export function ChatConversationPage(props: ChatConversationPageProps) {
  return <ConversationSurface {...props} mode="interactive" />
}

// Keep the exported component signature easy to inspect in downstream composition tests.
export type ChatConversationPageComponentProps = ComponentProps<typeof ChatConversationPage>
