import { describe, expect, it } from 'vitest'
import {
  getFinalMessageContent,
  getMessageContentAfterDelta
} from '../../features/agentRun/messageTimeline'

describe('messageTimeline', () => {
  it('streams into an initially empty assistant message', () => {
    expect(getMessageContentAfterDelta('', 'Hello')).toBe('Hello')
  })

  it('treats Chinese prose as content instead of a hidden status sentinel', () => {
    expect(getMessageContentAfterDelta('正在思考...', '下一段')).toBe('正在思考...下一段')
  })

  it('preserves current content unless the terminal event supplies final content', () => {
    expect(getFinalMessageContent('streamed')).toBe('streamed')
    expect(getFinalMessageContent('streamed', 'final')).toBe('final')
  })
})
