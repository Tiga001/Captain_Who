import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import {
  HUMAN_INTERACTION_MAX_INPUT_BYTES,
  parseHumanInteractionSettingsGetInput,
  parseHumanInteractionSettings,
  parseHumanInteractionSettingsUpdate,
  parseHumanInteractionToolInput,
  parseHumanInteractionQuestion,
  parseHumanInteractionAnswer,
  parseHumanInteractionResponse,
  parseHumanInteractionDelivery,
  parseHumanInteractionRequestSnapshot,
  parseHumanInteractionSubmitInput,
  parseHumanInteractionIgnoreInput,
  parseHumanInteractionListInput,
  parseHumanInteractionListOutput
} from './humanInteraction'

const fixture = JSON.parse(
  readFileSync(
    resolve(process.cwd(), 'packages/protocol/fixtures/human-interaction-v1.json'),
    'utf8'
  )
)

describe('human interaction wire contract', () => {
  it('round-trips the Rust-facing settings and complete question batch without UI state', () => {
    expect(parseHumanInteractionSettingsGetInput({})).toEqual({})
    expect(parseHumanInteractionSettings(fixture.settings)).toEqual(fixture.settings)
    expect(parseHumanInteractionSettingsUpdate(fixture.settingsUpdate)).toEqual(
      fixture.settingsUpdate
    )
    expect(parseHumanInteractionToolInput(fixture.toolInput)).toEqual(fixture.toolInput)
    expect(parseHumanInteractionRequestSnapshot(fixture.request)).toEqual(fixture.request)
    expect(parseHumanInteractionSubmitInput(fixture.submit)).toEqual(fixture.submit)
    expect(parseHumanInteractionIgnoreInput(fixture.ignore)).toEqual(fixture.ignore)
    expect(parseHumanInteractionListInput(fixture.listInput)).toEqual(fixture.listInput)
    expect(parseHumanInteractionListOutput({ items: [fixture.request], nextCursor: null })).toEqual(
      { items: [fixture.request], nextCursor: null }
    )
  })

  it('accepts more than three questions while enforcing payload and UTF-8 byte limits', () => {
    const questions = Array.from({ length: 20 }, (_, index) => ({ title: `Question ${index}` }))
    expect(parseHumanInteractionToolInput({ questions }).questions).toHaveLength(20)
    expect(() => parseHumanInteractionToolInput({ questions: [] })).toThrow()
    expect(() =>
      parseHumanInteractionToolInput({ questions: [{ title: '文'.repeat(2731) }] })
    ).toThrow()
    expect(() =>
      parseHumanInteractionToolInput({
        questions: Array.from({ length: 40 }, () => ({ title: 'x'.repeat(8192) }))
      })
    ).toThrow()
    expect(() =>
      parseHumanInteractionSubmitInput({
        ...fixture.submit,
        answers: [{ kind: 'text', questionId: 'q', text: '文'.repeat(10923) }]
      })
    ).toThrow()
    expect(() =>
      parseHumanInteractionToolInput({ questions: [{ title: 'q', options: ['yes', ' yes '] }] })
    ).toThrow()
    expect(() =>
      parseHumanInteractionToolInput({ questions: [{ title: 'q', options: [] }] })
    ).toThrow()
    expect(HUMAN_INTERACTION_MAX_INPUT_BYTES).toBe(262144)
  })

  it.each([NaN, Infinity, -1, 0.5, Number.MAX_SAFE_INTEGER + 1, '1'])(
    'rejects unsafe numbers %s',
    (value) => {
      expect(() =>
        parseHumanInteractionSettings({ ...fixture.settings, revision: value })
      ).toThrow()
      expect(() =>
        parseHumanInteractionSettings({ ...fixture.settings, updatedAt: value })
      ).toThrow()
      expect(() =>
        parseHumanInteractionSettingsUpdate({ ...fixture.settingsUpdate, expectedRevision: value })
      ).toThrow()
      expect(() =>
        parseHumanInteractionRequestSnapshot({ ...fixture.request, policyRevision: value })
      ).toThrow()
      expect(() =>
        parseHumanInteractionRequestSnapshot({ ...fixture.request, sequence: value })
      ).toThrow()
    }
  )

  it('preserves Host batch sequence independently of creation time and rejects client sequence', () => {
    const newer = { ...fixture.request, requestId: 'question-request-2', sequence: 2 }
    const page = parseHumanInteractionListOutput({
      items: [newer, fixture.request],
      nextCursor: null
    })
    expect(page.items.map((item) => item.createdAt)).toEqual([10, 10])
    expect(page.items.map((item) => item.sequence)).toEqual([2, 1])
    expect(() =>
      parseHumanInteractionListOutput({ items: [fixture.request, newer], nextCursor: null })
    ).toThrow()
    expect(() =>
      parseHumanInteractionListOutput({
        items: [{ ...newer, sequence: 1 }, fixture.request],
        nextCursor: null
      })
    ).toThrow()
    const missingSequence = { ...fixture.request }
    delete missingSequence.sequence
    expect(() => parseHumanInteractionRequestSnapshot(missingSequence)).toThrow()
    expect(() =>
      parseHumanInteractionRequestSnapshot({ ...fixture.request, sequence: 0 })
    ).toThrow()
    expect(() => parseHumanInteractionToolInput({ ...fixture.toolInput, sequence: 1 })).toThrow()
    expect(() => parseHumanInteractionSubmitInput({ ...fixture.submit, sequence: 1 })).toThrow()
  })

  it('rejects omitted nullable keys, untrusted fields, duplicate identities and answer ambiguity', () => {
    expect(() => parseHumanInteractionSettingsGetInput({ enabled: true })).toThrow()
    expect(() =>
      parseHumanInteractionSettings({ ...fixture.settings, prompt: 'override' })
    ).toThrow()
    expect(() =>
      parseHumanInteractionSettingsUpdate({ ...fixture.settingsUpdate, enabled: 'false' })
    ).toThrow()
    expect(() =>
      parseHumanInteractionRequestSnapshot({ ...fixture.request, response: undefined })
    ).toThrow()
    expect(() =>
      parseHumanInteractionRequestSnapshot({
        ...fixture.request,
        questions: [{ ...fixture.request.questions[0], options: undefined }]
      })
    ).toThrow()
    expect(() =>
      parseHumanInteractionRequestSnapshot({
        ...fixture.request,
        questions: [fixture.request.questions[0], fixture.request.questions[0]]
      })
    ).toThrow()
    expect(() =>
      parseHumanInteractionQuestion({ ...fixture.request.questions[0], rawArguments: {} })
    ).toThrow()
    expect(() =>
      parseHumanInteractionAnswer({ kind: 'skipped', questionId: 'q', text: 'hidden draft' })
    ).toThrow()
    expect(() =>
      parseHumanInteractionIgnoreInput({ ...fixture.ignore, answers: fixture.submit.answers })
    ).toThrow()
    expect(() =>
      parseHumanInteractionSubmitInput({
        ...fixture.submit,
        answers: [fixture.submit.answers[0], fixture.submit.answers[0]]
      })
    ).toThrow()
    expect(() =>
      parseHumanInteractionListInput({ ...fixture.listInput, cursor: undefined })
    ).toThrow()
    expect(() => parseHumanInteractionListInput({ ...fixture.listInput, limit: 101 })).toThrow()
    expect(() =>
      parseHumanInteractionListOutput({ items: [], nextCursor: null, internalQuery: 'secret' })
    ).toThrow()
    expect(() => parseHumanInteractionToolInput({ ...fixture.toolInput, mode: 'sync' })).toThrow()
    expect(() =>
      parseHumanInteractionSubmitInput({ ...fixture.submit, conversationId: 'bad\u0085id' })
    ).toThrow()
  })

  it('treats page cursors as opaque UTF-8 strings with their own 2048-byte bound', () => {
    const cursor = 'x'.repeat(2048)
    expect(parseHumanInteractionListInput({ ...fixture.listInput, cursor }).cursor).toBe(cursor)
    expect(parseHumanInteractionListOutput({ items: [], nextCursor: cursor }).nextCursor).toBe(
      cursor
    )
    expect(() =>
      parseHumanInteractionListInput({ ...fixture.listInput, cursor: `${cursor}x` })
    ).toThrow()
    expect(() => parseHumanInteractionListOutput({ items: [], nextCursor: `${cursor}x` })).toThrow()
    expect(() =>
      parseHumanInteractionListInput({ ...fixture.listInput, cursor: '文'.repeat(683) })
    ).toThrow()
  })

  it('keeps response and delivery separate and validates their immutable identities', () => {
    const response = {
      responseId: 'response-1',
      requestId: fixture.request.requestId,
      submissionId: fixture.submit.submissionId,
      kind: 'submitted',
      answers: fixture.submit.answers,
      createdAt: 20
    }
    const delivery = {
      responseId: 'response-1',
      status: 'pending',
      revision: 0,
      targetRunId: null,
      userMessageId: null,
      errorCode: null
    }
    const submitted = {
      ...fixture.request,
      status: 'submitted',
      revision: 1,
      updatedAt: 20,
      response,
      delivery
    }
    expect(parseHumanInteractionResponse(response)).toEqual(response)
    expect(parseHumanInteractionDelivery(delivery)).toEqual(delivery)
    expect(parseHumanInteractionRequestSnapshot(submitted)).toEqual(submitted)
    expect(() => parseHumanInteractionRequestSnapshot({ ...submitted, delivery: null })).toThrow()
    const cancelledDelivery = {
      ...submitted,
      delivery: { ...delivery, status: 'cancelled', revision: 1 }
    }
    expect(parseHumanInteractionRequestSnapshot(cancelledDelivery)).toEqual(cancelledDelivery)
    expect(() =>
      parseHumanInteractionRequestSnapshot({ ...cancelledDelivery, status: 'cancelled' })
    ).toThrow()
    expect(
      parseHumanInteractionRequestSnapshot({ ...fixture.request, status: 'cancelled' }).status
    ).toBe('cancelled')
    expect(() =>
      parseHumanInteractionRequestSnapshot({
        ...submitted,
        response: { ...response, requestId: 'other' }
      })
    ).toThrow()
    expect(() =>
      parseHumanInteractionRequestSnapshot({
        ...submitted,
        delivery: { ...delivery, responseId: 'other' }
      })
    ).toThrow()
    expect(() =>
      parseHumanInteractionRequestSnapshot({
        ...submitted,
        response: { ...response, answers: [fixture.submit.answers[0]] }
      })
    ).toThrow()
    expect(() =>
      parseHumanInteractionRequestSnapshot({
        ...submitted,
        response: {
          ...response,
          answers: [
            { kind: 'option', questionId: 'question-1', optionId: 'wrong' },
            fixture.submit.answers[1]
          ]
        }
      })
    ).toThrow()
    expect(() => parseHumanInteractionDelivery({ ...delivery, userMessageId: undefined })).toThrow()
    expect(() =>
      parseHumanInteractionRequestSnapshot({ ...fixture.request, status: 'submitted' })
    ).toThrow()
    const ignored = {
      ...fixture.request,
      status: 'ignored',
      revision: 1,
      updatedAt: 20,
      response: { ...response, kind: 'ignored', answers: [] },
      delivery: null
    }
    expect(parseHumanInteractionRequestSnapshot(ignored)).toEqual(ignored)
    expect(() => parseHumanInteractionRequestSnapshot({ ...ignored, mode: 'sync' })).toThrow()
    expect(() =>
      parseHumanInteractionResponse({ ...ignored.response, answers: fixture.submit.answers })
    ).toThrow()
  })
})
