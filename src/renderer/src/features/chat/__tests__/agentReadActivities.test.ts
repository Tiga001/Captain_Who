import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import { applyAgentEventToChatMessage } from '../../../app/agentEventReducer'
import type { ChatMessage } from '../chatTypes'
import { normalizeReadActivities, normalizeReadImageThumbnailDataUrl } from '../agentReadActivities'

const THUMBNAIL_DATA_URL =
  'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII='

function assistantMessage(): ChatMessage {
  return {
    id: 'assistant-message',
    role: 'assistant',
    content: '',
    createdAt: 1,
    status: 'pending'
  }
}

function readImageCall(): AgentToolCall {
  return {
    id: 'read-image-call',
    tool: 'read_image',
    args: { path: 'preview.png' },
    approvalStatus: 'not_required'
  }
}

function reduceReadImageResult(result: AgentToolResult) {
  const call = readImageCall()
  const called = applyAgentEventToChatMessage(assistantMessage(), {
    type: 'tool_call',
    runId: 'run-1',
    call
  })
  return applyAgentEventToChatMessage(called, {
    type: 'tool_result',
    runId: 'run-1',
    result
  })
}

describe('read_image presentation activity', () => {
  it('retains the backend-generated bounded thumbnail through the event reducer', () => {
    const message = reduceReadImageResult({
      callId: 'read-image-call',
      tool: 'read_image',
      ok: true,
      result: {
        path: 'preview.png',
        mimeType: 'image/png',
        thumbnailDataUrl: THUMBNAIL_DATA_URL
      }
    })

    expect(message.agentRun?.readActivities).toEqual([
      expect.objectContaining({
        callId: 'read-image-call',
        path: 'preview.png',
        status: 'completed',
        thumbnailDataUrl: THUMBNAIL_DATA_URL,
        fullDataUrl: undefined
      })
    ])
  })

  it('does not derive presentation state from the runtime-only original image payload', () => {
    const message = reduceReadImageResult({
      callId: 'read-image-call',
      tool: 'read_image',
      ok: true,
      result: {
        path: 'preview.png',
        mimeType: 'image/png',
        image: {
          mimeType: 'image/png',
          dataBase64: 'aGVsbG8='
        }
      }
    })

    expect(message.agentRun?.readActivities?.[0]).toEqual(
      expect.objectContaining({
        thumbnailDataUrl: undefined,
        fullDataUrl: undefined
      })
    )
    expect(JSON.stringify(message.agentRun?.readActivities)).not.toContain('aGVsbG8=')
  })

  it.each(['[binary/base64 omitted]', '[redacted]'])(
    'rejects the %s placeholder instead of constructing an image data URL',
    (placeholder) => {
      const message = reduceReadImageResult({
        callId: 'read-image-call',
        tool: 'read_image',
        ok: true,
        result: {
          path: 'preview.png',
          mimeType: 'image/png',
          thumbnailDataUrl: placeholder,
          image: {
            mimeType: 'image/png',
            dataBase64: placeholder
          }
        }
      })

      expect(message.agentRun?.readActivities?.[0]?.thumbnailDataUrl).toBeUndefined()
      expect(message.agentRun?.readActivities?.[0]?.fullDataUrl).toBeUndefined()
    }
  )

  it('rejects placeholder and oversized data URLs during persisted-run normalization', () => {
    const call = readImageCall()
    const normalized = normalizeReadActivities({
      runId: 'run-1',
      status: 'completed',
      toolDefinitions: [],
      toolCalls: [call],
      toolResults: [],
      approvals: [],
      diffs: [],
      timeline: [],
      readActivities: [
        {
          callId: call.id,
          tool: call.tool,
          kind: 'image',
          status: 'completed',
          path: 'preview.png',
          fileName: 'preview.png',
          thumbnailDataUrl: 'data:image/png;base64,[binary/base64 omitted]',
          fullDataUrl: 'data:image/png;base64,aGVsbG8=',
          updatedAt: 1
        }
      ]
    })

    expect(normalized[0]?.thumbnailDataUrl).toBeUndefined()
    expect(normalized[0]?.fullDataUrl).toBeUndefined()
    expect(
      normalizeReadImageThumbnailDataUrl(`data:image/png;base64,${'A'.repeat(600 * 1024)}`)
    ).toBeUndefined()
  })
})
