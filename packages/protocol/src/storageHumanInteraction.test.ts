import { describe, expect, it } from 'vitest'
import type { HumanInteractionResponseDisplay } from './humanInteraction'
import {
  assertNoHumanInteractionMessageProof,
  parseStorageHumanInteractionResponse
} from './storageHumanInteraction'

const response: HumanInteractionResponseDisplay = {
  type: 'human_interaction_response',
  schemaVersion: 1,
  requestId: 'request',
  responseId: 'response',
  answers: [{ kind: 'text', questionId: 'q', question: 'Which?', answer: 'Blue' }]
}
const message = {
  role: 'user',
  content: JSON.stringify(response),
  humanInteractionResponse: response
}

describe('stored human interaction display proof', () => {
  it('accepts complete bound output independently of JSON key order but never infers proof from text', () => {
    expect(parseStorageHumanInteractionResponse(message)).toEqual(response)
    expect(
      parseStorageHumanInteractionResponse({
        ...message,
        content: JSON.stringify(Object.fromEntries(Object.entries(response).reverse()))
      })
    ).toEqual(response)
    expect(
      parseStorageHumanInteractionResponse({ role: 'user', content: message.content })
    ).toBeUndefined()
  })
  it.each([
    { ...message, role: 'assistant' },
    { ...message, content: '{"type":"human_interaction_response"}' },
    { ...message, humanInteractionResponse: { ...response, responseId: 'other' } },
    {
      ...message,
      humanInteractionResponse: {
        ...response,
        answers: [{ ...response.answers[0], answer: 'Red' }]
      }
    },
    { ...message, humanInteractionResponse: { ...response, forged: true } }
  ])('rejects malformed or mismatched proof: %j', (input) => {
    expect(() => parseStorageHumanInteractionResponse(input)).toThrow()
  })
  it('rejects attempts to write Host proof and Renderer metadata, including explicit null', () => {
    for (const key of ['humanInteractionResponse', 'humanInteractionDisplay'])
      for (const value of [null, response])
        expect(() => assertNoHumanInteractionMessageProof({ id: 'message', [key]: value })).toThrow(
          'read-only'
        )
    expect(() =>
      assertNoHumanInteractionMessageProof({ id: 'message', content: message.content })
    ).not.toThrow()
  })
})
