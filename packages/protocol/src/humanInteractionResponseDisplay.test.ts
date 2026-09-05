import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import { parseAgentEventForHost } from './agentParsers/events'
import {
  HUMAN_INTERACTION_MAX_DISPLAY_BYTES,
  HUMAN_INTERACTION_MAX_INPUT_BYTES,
  parseHumanInteractionResponseDisplay
} from './humanInteraction'

const fixture = parseHumanInteractionResponseDisplay(
  JSON.parse(
    readFileSync(
      resolve(
        process.cwd(),
        'packages/protocol/fixtures/human-interaction-response-display-v1.json'
      ),
      'utf8'
    )
  )
)

describe('shared human answer history display', () => {
  it('preserves complete ordered titles and resolved answers for ToolResult and User content', () => {
    expect(parseHumanInteractionResponseDisplay(fixture)).toEqual(fixture)
    expect(parseHumanInteractionResponseDisplay(JSON.parse(JSON.stringify(fixture)))).toEqual(
      fixture
    )
    const allSkipped = {
      ...fixture,
      answers: fixture.answers.map(({ questionId, question }) => ({
        kind: 'skipped',
        questionId,
        question,
        answer: '已跳过'
      }))
    }
    expect(parseHumanInteractionResponseDisplay(allSkipped)).toEqual(allSkipped)
  })

  it.each([
    { ...fixture, type: 'user_message' },
    { ...fixture, schemaVersion: 2 },
    { ...fixture, mode: 'async' },
    { ...fixture, requestId: ' request' },
    { ...fixture, responseId: 'response\n' },
    { ...fixture, answers: [] },
    { ...fixture, answers: [fixture.answers[0], fixture.answers[0]] },
    { ...fixture, answers: [{ ...fixture.answers[0], text: 'hidden' }] },
    { ...fixture, answers: [{ ...fixture.answers[1], optionId: 'hidden' }] },
    { ...fixture, answers: [{ ...fixture.answers[2], answer: 'ignored' }] },
    { ...fixture, answers: [{ ...fixture.answers[1], answer: ' \n' }] },
    { ...fixture, answers: [{ ...fixture.answers[1], question: 'x\0y' }] }
  ])('rejects malformed or ambiguous display material %#', (value) => {
    expect(() => parseHumanInteractionResponseDisplay(value)).toThrow()
  })

  it('uses UTF-8 field limits without truncation or a product question count cap', () => {
    for (const answer of [
      { ...fixture.answers[0], answer: '文'.repeat(683) },
      { ...fixture.answers[1], answer: '文'.repeat(10_923) },
      { ...fixture.answers[2], question: '文'.repeat(2_731) }
    ]) {
      expect(() =>
        parseHumanInteractionResponseDisplay({ ...fixture, answers: [answer] })
      ).toThrow()
    }
    const answers = Array.from({ length: 100 }, (_, index) => ({
      ...fixture.answers[2],
      questionId: `question-${index}`
    }))
    expect(parseHumanInteractionResponseDisplay({ ...fixture, answers }).answers).toHaveLength(100)
  })

  it('allows a combined display above the input budget while bounding titles and answers separately', () => {
    const answers = Array.from({ length: 30 }, (_, index) => ({
      kind: 'text',
      questionId: `question-${index}`,
      question: 'q'.repeat(8_000),
      answer: 'a'.repeat(8_000)
    }))
    const value = { ...fixture, answers }
    const bytes = new TextEncoder().encode(JSON.stringify(value)).length
    expect(bytes).toBeGreaterThan(HUMAN_INTERACTION_MAX_INPUT_BYTES)
    expect(bytes).toBeLessThan(HUMAN_INTERACTION_MAX_DISPLAY_BYTES)
    expect(parseHumanInteractionResponseDisplay(value)).toEqual(value)
    for (const type of ['guidance_queued', 'guidance_applied'] as const) {
      const event = {
        type,
        runId: 'active-run',
        guidanceId: 'human-answer-guidance',
        clientMessageId: value.responseId,
        content: JSON.stringify(value),
        attachments: [],
        createdAt: 20,
        ...(type === 'guidance_applied' ? { sequence: 7 } : {})
      }
      expect(parseAgentEventForHost(event)).toEqual(event)
    }
    expect(() =>
      parseHumanInteractionResponseDisplay({
        ...value,
        answers: [
          ...answers,
          ...answers.slice(0, 5).map((answer, index) => ({
            ...answer,
            questionId: `extra-${index}`,
            answer: 'a'
          }))
        ]
      })
    ).toThrow()
    expect(() =>
      parseHumanInteractionResponseDisplay({
        ...value,
        answers: answers.map((answer) => ({ ...answer, question: 'q', answer: 'a'.repeat(9_000) }))
      })
    ).toThrow()
  })
})
