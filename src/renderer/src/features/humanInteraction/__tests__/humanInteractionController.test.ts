import { describe, expect, it } from 'vitest'
import { HumanInteractionController } from '../humanInteractionController'
import {
  humanInteractionResponseDisplay,
  mergeHumanInteractionRequest
} from '../humanInteractionState'
import { deferred, fakeHost, question, submitted } from './humanInteractionFixtures'
import type { HostInvocationResult } from '@mycopilot/host-api'
import type {
  HumanInteractionListOutput,
  HumanInteractionRequestSnapshot
} from '@mycopilot/protocol'

function fill(controller: HumanInteractionController, request: HumanInteractionRequestSnapshot) {
  for (const q of request.questions)
    controller.setAnswer(request.requestId, { kind: 'skipped', questionId: q.id })
}
describe('independent human interaction controller', () => {
  it('retains terminal tombstones and independently advances delivery without reviving cards', () => {
    const request = question(),
      answer = submitted(request)
    const bound = {
      ...answer,
      delivery: { ...answer.delivery!, status: 'bound' as const, revision: 1, targetRunId: 'run' }
    }
    const retry = { ...answer, delivery: { ...answer.delivery!, revision: 2 } }
    expect(mergeHumanInteractionRequest(bound, retry).delivery?.status).toBe('pending')
    const applied = {
      ...answer,
      delivery: { ...answer.delivery!, status: 'applied' as const, revision: 3 }
    }
    expect(mergeHumanInteractionRequest(applied, bound)).toBe(applied)
    expect(mergeHumanInteractionRequest(applied, { ...request, revision: 90 })).toBe(applied)
    expect(mergeHumanInteractionRequest(bound, applied).delivery?.status).toBe('applied')
    expect(humanInteractionResponseDisplay(answer)?.answers).toEqual([
      { questionId: 'question-one', question: 'Choose', kind: 'skipped', answer: '已跳过' },
      { questionId: 'question-two', question: 'Details', kind: 'skipped', answer: '已跳过' }
    ])
  })
  it('loads every page while late open snapshots cannot replace a live terminal notification', async () => {
    const request = question('old'),
      recent = question('recent', 2),
      host = fakeHost([request, recent])
    const page = deferred<HostInvocationResult<HumanInteractionListOutput>>()
    host.api.listRequests
      .mockResolvedValueOnce({ ok: true, value: { items: [recent], nextCursor: 'next' } })
      .mockReturnValueOnce(page.promise)
    const controller = new HumanInteractionController(host.api),
      disconnect = controller.connect()
    const refresh = controller.refresh('chat')
    await Promise.resolve()
    await Promise.resolve()
    host.notify(submitted(request))
    page.resolve({ ok: true, value: { items: [request], nextCursor: null } })
    await refresh
    expect(Object.keys(controller.getSnapshot().requests)).toHaveLength(2)
    expect(controller.getSnapshot().requests.old.status).toBe('submitted')
    expect(host.api.listRequests).toHaveBeenLastCalledWith({
      conversationId: 'chat',
      cursor: 'next',
      limit: 100
    })
    disconnect()
  })
  it('a complete refresh tombstones removed IDs but preserves a newer notification outside its cursor', async () => {
    const old = question('removed'),
      host = fakeHost(),
      controller = new HumanInteractionController(host.api)
    controller.merge(old)
    const pending = deferred<HostInvocationResult<HumanInteractionListOutput>>()
    host.api.listRequests.mockReturnValueOnce(pending.promise)
    const refresh = controller.refresh('chat')
    controller.merge(question('created-during-scan', 2))
    pending.resolve({ ok: true, value: { items: [], nextCursor: null } })
    await refresh
    controller.merge(old)
    expect(Object.keys(controller.getSnapshot().requests)).toEqual(['created-during-scan'])
  })
  it('late overlapping refreshes cannot replace a newer result or pagination status', async () => {
    const host = fakeHost([question()]),
      controller = new HumanInteractionController(host.api)
    const old = deferred<HostInvocationResult<HumanInteractionListOutput>>()
    host.api.listRequests.mockReturnValueOnce(old.promise)
    const first = controller.refresh('chat')
    await controller.refresh('chat')
    old.resolve({ ok: true, value: { items: [], nextCursor: null } })
    await first
    expect(controller.getSnapshot().requests.question.status).toBe('open')
    expect(controller.getSnapshot().loads.chat.status).toBe('ready')
  })
  it('retains pages and draft text across selection, minimization and newer async preemption', () => {
    const host = fakeHost(),
      controller = new HumanInteractionController(host.api)
    const old = question('old'),
      recent = question('recent', 3)
    controller.merge(old)
    controller.setPage('old', 1)
    controller.setAnswer('old', { kind: 'text', questionId: 'old-two', text: 'draft' })
    controller.minimize('old')
    controller.merge(recent)
    controller.open('old')
    controller.merge(question('older-history', 2))
    expect(controller.getSnapshot().selected.chat).toBe('old')
    expect(controller.getSnapshot().drafts.old).toEqual({
      pageIndex: 1,
      answers: { 'old-two': { kind: 'text', questionId: 'old-two', text: 'draft' } }
    })
    controller.merge(question('newest', 4))
    expect(controller.getSnapshot().selected.chat).toBe('newest')
  })
  it('deduplicates loading clicks and retries uncertain outcomes with the exact same immutable payload', async () => {
    const request = question(),
      host = fakeHost([request]),
      controller = new HumanInteractionController(host.api, () => 'stable-id')
    controller.merge(request)
    fill(controller, request)
    controller.setPage(request.requestId, 1)
    const pending = deferred<HostInvocationResult<HumanInteractionRequestSnapshot>>()
    host.api.submit.mockReturnValueOnce(pending.promise)
    const first = controller.submit(request.requestId),
      second = controller.submit(request.requestId)
    expect(second).toBe(first)
    await Promise.resolve()
    expect(host.api.submit).toHaveBeenCalledTimes(1)
    pending.reject(new Error('IPC connection closed'))
    await first
    const frozen = structuredClone(host.api.submit.mock.calls[0][0])
    controller.setAnswer(request.requestId, {
      kind: 'text',
      questionId: request.questions[1].id,
      text: 'must not replace uncertain payload'
    })
    await controller.ignore(request.requestId)
    expect(host.api.ignore).not.toHaveBeenCalled()
    expect(controller.getSnapshot().operations.question.isDraftLocked).toBe(true)
    expect(controller.getSnapshot().drafts.question.pageIndex).toBe(1)
    await controller.submit(request.requestId)
    expect(host.api.submit.mock.calls[1][0]).toEqual(frozen)
    expect(controller.getSnapshot().requests.question.status).toBe('submitted')
    expect(controller.getSnapshot().operations.question.error).toBeNull()
  })
  it('a different window settlement wins even if the original RPC later rejects', async () => {
    const request = question(),
      host = fakeHost([request]),
      controller = new HumanInteractionController(host.api)
    controller.merge(request)
    fill(controller, request)
    const pending = deferred<HostInvocationResult<HumanInteractionRequestSnapshot>>()
    host.api.submit.mockReturnValueOnce(pending.promise)
    const submission = controller.submit(request.requestId)
    await Promise.resolve()
    controller.merge(submitted(request))
    pending.reject(new Error('late lost response'))
    await submission
    expect(controller.getSnapshot().requests.question.status).toBe('submitted')
    expect(controller.getSnapshot().operations.question.error).toBeNull()
  })
  it('does not accept a terminal receipt bound to a different immutable Run and permits an exact retry', async () => {
    const request = question(),
      host = fakeHost([request]),
      controller = new HumanInteractionController(host.api)
    controller.merge(request)
    fill(controller, request)
    host.api.submit.mockResolvedValueOnce({
      ok: true,
      value: { ...submitted(request), runId: 'foreign-run' }
    })
    await controller.submit(request.requestId)
    expect(controller.getSnapshot().requests.question.status).toBe('open')
    expect(controller.getSnapshot().operations.question).toMatchObject({
      isSubmitting: false,
      isDraftLocked: true,
      error: 'outcome_unknown'
    })
    await controller.submit(request.requestId)
    expect(host.api.submit.mock.calls[0][0]).toEqual(host.api.submit.mock.calls[1][0])
    expect(controller.getSnapshot().requests.question.status).toBe('submitted')
  })
  it('a definitive rejection retains draft/page and a corrected submission gets a new identity', async () => {
    const request = question(),
      host = fakeHost([request])
    let sequence = 0
    const controller = new HumanInteractionController(host.api, () => `id-${++sequence}`)
    controller.merge(request)
    fill(controller, request)
    controller.setPage(request.requestId, 1)
    host.api.submit.mockResolvedValueOnce({
      ok: false,
      error: {
        message: 'Rejected',
        data: { type: 'human_interaction_error', code: 'invalid_input' }
      }
    })
    await controller.submit(request.requestId)
    expect(controller.getSnapshot().drafts.question.pageIndex).toBe(1)
    controller.setAnswer(request.requestId, {
      kind: 'text',
      questionId: request.questions[1].id,
      text: 'corrected'
    })
    await controller.submit(request.requestId)
    expect(host.api.submit.mock.calls.map(([input]) => input.submissionId)).toEqual([
      'id-1',
      'id-2'
    ])
  })
  it('ignore sends no draft and settles only that batch while skipped submit remains a response', async () => {
    const first = question('first'),
      second = question('second', 2),
      host = fakeHost([first, second]),
      controller = new HumanInteractionController(host.api)
    controller.merge(first)
    controller.merge(second)
    fill(controller, first)
    fill(controller, second)
    await controller.ignore('first')
    await controller.submit('second')
    expect(host.api.ignore.mock.calls[0][0]).not.toHaveProperty('answers')
    expect(controller.getSnapshot().requests.first.status).toBe('ignored')
    expect(controller.getSnapshot().requests.second.status).toBe('submitted')
    expect(humanInteractionResponseDisplay(controller.getSnapshot().requests.first)).toBeNull()
  })
  it('refuses incomplete, whitespace and out of scope option answers before any RPC', async () => {
    const request = question(),
      host = fakeHost([request]),
      controller = new HumanInteractionController(host.api)
    controller.merge(request)
    await controller.submit(request.requestId)
    controller.setAnswer(request.requestId, {
      kind: 'option',
      questionId: request.questions[0].id,
      optionId: 'foreign-option'
    })
    controller.setAnswer(request.requestId, {
      kind: 'text',
      questionId: request.questions[1].id,
      text: '  '
    })
    await controller.submit(request.requestId)
    expect(host.api.submit).not.toHaveBeenCalled()
    expect(
      controller.getSnapshot().drafts.question.answers[request.questions[0].id]
    ).toBeUndefined()
  })
})

it('reuses the submission identity for an unchanged payload even after a definitive rejection', async () => {
  const request = question(),
    host = fakeHost([request])
  let ids = 0
  const controller = new HumanInteractionController(host.api, () => `stable-${++ids}`)
  controller.merge(request)
  fill(controller, request)
  host.api.submit.mockResolvedValueOnce({
    ok: false,
    error: { message: 'Rejected', data: { type: 'human_interaction_error', code: 'invalid_input' } }
  })
  await controller.submit(request.requestId)
  await controller.submit(request.requestId)
  expect(host.api.submit.mock.calls[0][0]).toEqual(host.api.submit.mock.calls[1][0])
  expect(ids).toBe(1)
})
