import type { WorkflowInstance, WorkflowRecord } from '@mycopilot/protocol'

export interface BindingDraft {
  id: string
  name: string
  projectId: string | null
  color: string
  record: WorkflowRecord | null
  instance: WorkflowInstance | null
  bindings: Record<string, string | null>
  baseline: string
}
