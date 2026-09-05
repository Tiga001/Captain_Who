import { describe, expect, it } from 'vitest'
import {
  parseAgentManualContextCompactionOperation,
  parseAgentManualContextCompactionStartInput,
  parseAgentManualContextCompactionStatusOutput
} from './manualContextCompaction'

const operation = {
  schemaVersion: 1,
  conversationId: 'chat',
  requestId: 'request',
  operationId: 'operation',
  status: 'running',
  phase: 'generating',
  startedAt: 10,
  updatedAt: 20
}
describe('manual compaction boundary', () => {
  it('accepts a separate operation and rejects renderer-supplied runtime authority', () => {
    expect(parseAgentManualContextCompactionOperation(operation)).toEqual(operation)
    expect(() =>
      parseAgentManualContextCompactionStartInput({
        conversationId: 'chat',
        requestId: 'request',
        cursor: 9
      })
    ).toThrow()
    expect(() =>
      parseAgentManualContextCompactionOperation({ ...operation, continuation: 'secret' })
    ).toThrow()
  })
  it('requires terminal facts and an applied summary anchor', () => {
    expect(() =>
      parseAgentManualContextCompactionOperation({
        ...operation,
        status: 'completed',
        completedAt: 20
      })
    ).toThrow()
    const completed = {
      ...operation,
      status: 'completed',
      summaryId: 'summary',
      coveredThroughMessageId: 'assistant',
      completedAt: 20
    }
    expect(parseAgentManualContextCompactionOperation(completed)).toEqual(completed)
    expect(() =>
      parseAgentManualContextCompactionOperation({ ...operation, updatedAt: 1 })
    ).toThrow()
    expect(() =>
      parseAgentManualContextCompactionStatusOutput({ operations: [operation, operation] })
    ).toThrow()
  })
})
