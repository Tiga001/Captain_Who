import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import {
  AGENT_APPROVE_ACTION_METHOD,
  AGENT_CANCEL_ACTION_METHOD,
  AGENT_CANCEL_RUN_METHOD,
  AGENT_CLEAR_USAGE_RECORDS_METHOD,
  AGENT_EVENT_NOTIFICATION_METHOD,
  AGENT_GET_CONTEXT_WINDOW_SNAPSHOT_METHOD,
  AGENT_GET_FILE_WRITE_DIFF_METHOD,
  AGENT_GET_USAGE_SUMMARY_METHOD,
  AGENT_LIST_PENDING_ACTIONS_METHOD,
  AGENT_READ_FILE_DRAFT_METHOD,
  AGENT_REJECT_ACTION_METHOD,
  AGENT_START_CONVERSATION_TURN_METHOD,
  AGENT_STEER_RUN_METHOD,
  type AgentEvent
} from './agent'

const fixture = JSON.parse(
  readFileSync(resolve(process.cwd(), 'packages/protocol/fixtures/agent-contract-v1.json'), 'utf8')
)

const events: Record<string, AgentEvent> = {
  messageDelta: {
    type: 'message_delta',
    runId: 'run-contract-v1',
    streamId: 'stream-contract-v1',
    delta: 'hello'
  },
  toolCall: {
    type: 'tool_call',
    runId: 'run-contract-v1',
    call: {
      id: 'call-contract-v1',
      tool: 'read_file',
      args: { path: 'README.md' },
      approvalStatus: 'not_required',
      reason: 'Inspect project documentation.'
    }
  },
  toolResult: {
    type: 'tool_result',
    runId: 'run-contract-v1',
    result: {
      callId: 'call-contract-v1',
      tool: 'read_file',
      ok: true,
      result: { content: 'MyCopilot Next' }
    }
  },
  done: {
    type: 'done',
    runId: 'run-contract-v1',
    success: true,
    status: 'completed',
    content: 'done'
  }
}

describe('Agent cross-language golden contract', () => {
  it('keeps TypeScript event discriminants and field casing aligned with the fixture', () => {
    expect(events).toEqual(fixture.events)
  })

  it('keeps the JSON-RPC method namespace centralized', () => {
    expect(fixture.methods).toEqual({
      cancelRun: AGENT_CANCEL_RUN_METHOD,
      steerRun: AGENT_STEER_RUN_METHOD,
      startConversationTurn: AGENT_START_CONVERSATION_TURN_METHOD,
      getContextWindowSnapshot: AGENT_GET_CONTEXT_WINDOW_SNAPSHOT_METHOD,
      listPendingActions: AGENT_LIST_PENDING_ACTIONS_METHOD,
      approveAction: AGENT_APPROVE_ACTION_METHOD,
      rejectAction: AGENT_REJECT_ACTION_METHOD,
      cancelAction: AGENT_CANCEL_ACTION_METHOD,
      getUsageSummary: AGENT_GET_USAGE_SUMMARY_METHOD,
      clearUsageRecords: AGENT_CLEAR_USAGE_RECORDS_METHOD,
      readFileDraft: AGENT_READ_FILE_DRAFT_METHOD,
      getFileWriteDiff: AGENT_GET_FILE_WRITE_DIFF_METHOD,
      eventNotification: AGENT_EVENT_NOTIFICATION_METHOD
    })
  })
})
