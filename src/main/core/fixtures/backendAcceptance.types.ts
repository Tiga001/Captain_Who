export type AcceptanceWork = 'text' | 'approval' | 'stop'
export type AcceptanceAction = 'submit' | 'approve' | 'stop'

export interface AcceptanceSnapshot {
  conversationId: string | null
  assistantMessageId: string | null
  runId: string | null
  status: string | null
  content: string
  renderedContent: string
  renderedStatus: string | null
  pendingApprovals: number
  notices: string[]
}

export interface AcceptanceResult extends AcceptanceSnapshot {
  action: AcceptanceAction
  accepted: boolean
  receiptMs: number
  frameOpportunityMs: number
}

export interface AcceptanceControls {
  submit(work: AcceptanceWork, index: number): Promise<AcceptanceResult>
  approve(): Promise<AcceptanceResult>
  stop(): Promise<AcceptanceResult>
  snapshot(): AcceptanceSnapshot
  flushDiagnostics(): object[]
}
