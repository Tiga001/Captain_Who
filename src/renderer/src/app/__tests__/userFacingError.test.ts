import { describe, expect, it } from 'vitest'
import {
  getAgentInterruptionReason,
  getUserFacingErrorKey,
  getUserFacingErrorMessage
} from '../../errors/userFacingError'

describe('userFacingError', () => {
  it('never exposes an unstructured exception message', () => {
    const error = new Error('provider secret and implementation details')
    const t = (key: string) => `translated:${key}`

    expect(getUserFacingErrorKey(error, 'chat.conversationLoadFailed')).toBe(
      'chat.conversationLoadFailed'
    )
    expect(getUserFacingErrorMessage(error, t, 'chat.conversationLoadFailed')).toBe(
      'translated:chat.conversationLoadFailed'
    )
  })

  it('maps structured host error fields to a translation key', () => {
    const error = {
      data: {
        type: 'stateConflict',
        code: 'revisionConflict'
      }
    }

    expect(
      getUserFacingErrorKey(error, 'chat.conversationLoadFailed', {
        codes: { revisionConflict: 'agentTemplates.operationFailed' }
      })
    ).toBe('agentTemplates.operationFailed')
  })

  it('classifies invalid streamed tool arguments as an invalid response', () => {
    expect(
      getAgentInterruptionReason({
        data: { code: 'invalid_stream_tool_arguments' }
      })
    ).toBe('response_invalid')
  })

  it('uses the generic request failure for an unknown structured error', () => {
    expect(getAgentInterruptionReason({ data: { code: 'future_error' } })).toBe('request_failed')
  })
})
