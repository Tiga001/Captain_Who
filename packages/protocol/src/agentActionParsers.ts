import type { AgentApproveActionRequest } from './agent'
import { expectEnum, expectNonEmptyString, expectOnlyKeys, expectRecord } from './skills/validation'

function expectTrimmedIdentifier(value: unknown, context: string): string {
  const identifier = expectNonEmptyString(value, context)
  if (identifier.trim() !== identifier) {
    throw new Error(`Invalid ${context}: expected a trimmed identifier`)
  }
  return identifier
}

export function parseAgentApproveActionRequest(value: unknown): AgentApproveActionRequest {
  const context = 'Agent approve action request'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['runId', 'actionId', 'approvalScope'] as const, context)

  return {
    runId: expectTrimmedIdentifier(record.runId, `${context}.runId`),
    actionId: expectTrimmedIdentifier(record.actionId, `${context}.actionId`),
    approvalScope: expectEnum(
      record.approvalScope,
      ['singleAction', 'remainingApplyPatchInRun'] as const,
      `${context}.approvalScope`
    )
  }
}
