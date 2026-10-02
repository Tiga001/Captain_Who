import { createContext, useContext, type ReactNode } from 'react'

const WorkflowNavigationContext = createContext<((instanceId: string) => void) | null>(null)

export function WorkflowNavigationProvider({
  children,
  onOpenWorkflow
}: {
  children: ReactNode
  onOpenWorkflow: (instanceId: string) => void
}) {
  return (
    <WorkflowNavigationContext.Provider value={onOpenWorkflow}>
      {children}
    </WorkflowNavigationContext.Provider>
  )
}

export function useWorkflowNavigation() {
  return useContext(WorkflowNavigationContext)
}
