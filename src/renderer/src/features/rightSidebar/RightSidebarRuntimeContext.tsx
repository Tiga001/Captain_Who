import { createContext, useContext } from 'react'

interface RightSidebarRuntimeContextValue {
  activeConversationId: string | null
  activeWorkspaceKey: string | null
}

export const RightSidebarRuntimeContext = createContext<RightSidebarRuntimeContextValue>({
  activeConversationId: null,
  activeWorkspaceKey: null
})

export function useRightSidebarRuntimeContext(): RightSidebarRuntimeContextValue {
  return useContext(RightSidebarRuntimeContext)
}
