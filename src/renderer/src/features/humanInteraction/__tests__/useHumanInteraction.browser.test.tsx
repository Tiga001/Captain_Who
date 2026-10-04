import { expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { HumanInteractionHostApi } from '@mycopilot/host-api'
import { deferred, fakeHost, question } from './humanInteractionFixtures'
import type { HostInvocationResult } from '@mycopilot/host-api'
import type { HumanInteractionListOutput } from '@mycopilot/protocol'
vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))
vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))
import { useHumanInteraction, type HumanInteractionControllerView } from '../useHumanInteraction'

function Harness({
  api,
  conversationId = 'chat',
  hasApproval = false,
  readOnly = false,
  onRender
}: {
  api: HumanInteractionHostApi
  conversationId?: string
  hasApproval?: boolean
  readOnly?: boolean
  onRender(value: HumanInteractionControllerView): void
}) {
  const result = useHumanInteraction({ api, conversationId, hasApproval, readOnly })
  onRender(result)
  return (
    <output data-testid="active">
      {result.activeBatch?.requestId ?? 'none'}:{result.activeDraft.pageIndex}
    </output>
  )
}

it('arbitrates approval > sync > newest async while preserving older drafts across chat changes', async () => {
  const old = question('old'),
    host = fakeHost([old]),
    api = host.api
  let result!: HumanInteractionControllerView
  const onRender = (value: HumanInteractionControllerView) => {
    result = value
  }
  const screen = await render(<Harness api={api} onRender={onRender} />)
  await expect.poll(() => result.activeBatch?.requestId).toBe('old')
  result.setAnswer('old', { kind: 'text', questionId: 'old-two', text: 'saved draft' })
  result.setPage('old', 1)
  result.minimize('old')
  await expect.poll(() => result.activeBatch).toBeNull()
  host.notify(question('new', 2))
  await expect.poll(() => result.activeBatch?.requestId).toBe('new')
  const blocking = { ...question('blocking', 3), mode: 'sync' as const }
  host.notify(blocking)
  await expect.poll(() => result.activeBatch?.requestId).toBe('blocking')
  result.open('old')
  expect(result.activeBatch?.requestId).toBe('blocking')
  await screen.rerender(<Harness api={api} onRender={onRender} hasApproval />)
  await expect.poll(() => result.activeBatch).toBeNull()
  result.open('old')
  await result.submit('blocking')
  expect(api.submit).not.toHaveBeenCalled()
  host.notify({ ...blocking, status: 'cancelled', revision: 1, updatedAt: 20 })
  await screen.rerender(<Harness api={api} onRender={onRender} />)
  await expect.poll(() => result.activeBatch?.requestId).toBe('new')
  result.open('old')
  await expect.poll(() => result.activeBatch?.requestId).toBe('old')
  expect(result.activeDraft.pageIndex).toBe(1)
  expect(result.activeDraft.answers['old-two']).toMatchObject({ text: 'saved draft' })
  await screen.rerender(<Harness api={api} onRender={onRender} conversationId="other" />)
  await expect.poll(() => result.status).toBe('ready')
  await screen.rerender(<Harness api={api} onRender={onRender} />)
  await expect.poll(() => result.activeBatch?.requestId).toBe('old')
  expect(result.activeDraft.pageIndex).toBe(1)
  await screen.unmount()
})

it('refreshes on window focus and Core reconnect without restoring another window settled batch', async () => {
  const request = question(),
    host = fakeHost([request])
  let result!: HumanInteractionControllerView
  const screen = await render(
    <Harness
      api={host.api}
      onRender={(value) => {
        result = value
      }}
    />
  )
  await expect.poll(() => result.activeBatch?.requestId).toBe(request.requestId)
  host.database.set(request.requestId, {
    ...request,
    status: 'cancelled',
    revision: 1,
    updatedAt: 20
  })
  window.dispatchEvent(new Event('focus'))
  await expect.poll(() => result.activeBatch).toBeNull()
  host.notify(request)
  await expect.poll(() => result.requests[0].status).toBe('cancelled')
  host.database.set('next', question('next', 2))
  for (const resync of host.resyncListeners) resync()
  await expect.poll(() => result.activeBatch?.requestId).toBe('next')
  await screen.unmount()
  expect(host.requestListeners.size).toBe(0)
  expect(host.resyncListeners.size).toBe(0)
})

it('resync reads only the visible conversation and revisiting cached conversations refreshes them on demand', async () => {
  const host = fakeHost([question()])
  let result!: HumanInteractionControllerView
  const onRender = (value: HumanInteractionControllerView) => {
    result = value
  }
  const screen = await render(<Harness api={host.api} onRender={onRender} />)
  await expect.poll(() => result.activeBatch?.requestId).toBe('question')
  for (let index = 0; index < 8; index += 1) {
    await screen.rerender(
      <Harness api={host.api} onRender={onRender} conversationId={`visited-${index}`} />
    )
    await expect.poll(() => result.status).toBe('ready')
  }
  host.database.set('question', { ...question(), status: 'cancelled', revision: 1, updatedAt: 20 })
  host.api.listRequests.mockClear()
  for (const resync of host.resyncListeners) resync()
  await expect.poll(() => host.api.listRequests.mock.calls.length).toBe(1)
  expect(host.api.listRequests.mock.calls[0][0].conversationId).toBe('visited-7')
  await screen.rerender(<Harness api={host.api} onRender={onRender} />)
  await expect.poll(() => result.requests[0]?.status).toBe('cancelled')
  expect(result.activeBatch).toBeNull()
  await screen.unmount()
})

it('defers reconnect reads while hidden and refreshes on becoming visible', async () => {
  const host = fakeHost([question()])
  let result!: HumanInteractionControllerView
  const visibility = vi.spyOn(document, 'visibilityState', 'get')
  const screen = await render(
    <Harness
      api={host.api}
      onRender={(value) => {
        result = value
      }}
    />
  )
  try {
    await expect.poll(() => result.activeBatch?.requestId).toBe('question')
    host.api.listRequests.mockClear()
    visibility.mockReturnValue('hidden')
    for (const resync of host.resyncListeners) resync()
    window.dispatchEvent(new Event('focus'))
    document.dispatchEvent(new Event('visibilitychange'))
    expect(host.api.listRequests).not.toHaveBeenCalled()
    host.database.set('question', {
      ...question(),
      status: 'cancelled',
      revision: 1,
      updatedAt: 20
    })
    visibility.mockReturnValue('visible')
    document.dispatchEvent(new Event('visibilitychange'))
    await expect.poll(() => result.requests[0]?.status).toBe('cancelled')
    expect(host.api.listRequests).toHaveBeenCalledTimes(1)
  } finally {
    visibility.mockRestore()
    await screen.unmount()
  }
})

it('shares in-flight reads between hook owners and applies the final refresh after a reconnect burst', async () => {
  const host = fakeHost([question()])
  const first = deferred<HostInvocationResult<HumanInteractionListOutput>>()
  const latest = deferred<HostInvocationResult<HumanInteractionListOutput>>()
  host.api.listRequests.mockReturnValueOnce(first.promise).mockReturnValueOnce(latest.promise)
  let left!: HumanInteractionControllerView, right!: HumanInteractionControllerView
  const screen = await render(
    <>
      <Harness
        api={host.api}
        onRender={(value) => {
          left = value
        }}
      />
      <Harness
        api={host.api}
        onRender={(value) => {
          right = value
        }}
      />
    </>
  )
  expect(host.api.listRequests).toHaveBeenCalledTimes(1)
  for (let index = 0; index < 80; index += 1) for (const resync of host.resyncListeners) resync()
  expect(host.api.listRequests).toHaveBeenCalledTimes(1)
  first.resolve({ ok: true, value: { items: [question()], nextCursor: null } })
  await expect.poll(() => host.api.listRequests.mock.calls.length).toBe(2)
  expect(left.canInteract).toBe(false)
  expect(right.canInteract).toBe(false)
  const settled = { ...question(), status: 'cancelled' as const, revision: 1, updatedAt: 20 }
  latest.resolve({ ok: true, value: { items: [settled], nextCursor: null } })
  await expect.poll(() => left.canInteract && right.canInteract).toBe(true)
  expect(left.activeBatch).toBeNull()
  expect(right.activeBatch).toBeNull()
  expect(left.requests[0].status).toBe('cancelled')
  expect(right.requests[0].status).toBe('cancelled')
  expect(host.api.listRequests).toHaveBeenCalledTimes(2)
  await screen.unmount()
})

it('requires the trailing post-approval scan when approval changes during an existing read', async () => {
  const host = fakeHost([question()])
  const before = deferred<HostInvocationResult<HumanInteractionListOutput>>()
  const after = deferred<HostInvocationResult<HumanInteractionListOutput>>()
  host.api.listRequests.mockReturnValueOnce(before.promise).mockReturnValueOnce(after.promise)
  let result!: HumanInteractionControllerView
  const onRender = (value: HumanInteractionControllerView) => {
    result = value
  }
  const screen = await render(<Harness api={host.api} onRender={onRender} />)
  await screen.rerender(<Harness api={host.api} onRender={onRender} hasApproval />)
  await screen.rerender(<Harness api={host.api} onRender={onRender} />)
  before.resolve({ ok: true, value: { items: [question()], nextCursor: null } })
  await expect.poll(() => host.api.listRequests.mock.calls.length).toBe(2)
  expect(result.canInteract).toBe(false)
  expect(result.activeBatch).toBeNull()
  after.resolve({ ok: true, value: { items: [question()], nextCursor: null } })
  await expect.poll(() => result.activeBatch?.requestId).toBe('question')
  await screen.unmount()
})

it('retains in-memory draft and selection after the panel owner unmounts and remounts', async () => {
  const host = fakeHost([question()])
  let result!: HumanInteractionControllerView
  const onRender = (value: HumanInteractionControllerView) => {
    result = value
  }
  let screen = await render(<Harness api={host.api} onRender={onRender} />)
  await expect.poll(() => result.activeBatch?.requestId).toBe('question')
  result.setAnswer('question', {
    kind: 'text',
    questionId: 'question-two',
    text: 'kept across hide'
  })
  result.setPage('question', 1)
  await screen.unmount()
  screen = await render(<Harness api={host.api} onRender={onRender} />)
  await expect.poll(() => result.activeDraft.pageIndex).toBe(1)
  expect(result.activeDraft.answers['question-two']).toMatchObject({ text: 'kept across hide' })
  await screen.unmount()
})

it('child observer mode exposes no actionable panel and cannot issue mutation or query calls', async () => {
  const host = fakeHost([question()])
  let result!: HumanInteractionControllerView
  const screen = await render(
    <Harness
      api={host.api}
      readOnly
      onRender={(value) => {
        result = value
      }}
    />
  )
  await expect.poll(() => result.canInteract).toBe(false)
  result.open('question')
  result.setPage('question', 1)
  result.setAnswer('question', { kind: 'skipped', questionId: 'question-one' })
  await result.submit('question')
  await result.ignore('question')
  expect(result.activeBatch).toBeNull()
  expect(host.api.listRequests).not.toHaveBeenCalled()
  expect(host.api.submit).not.toHaveBeenCalled()
  expect(host.api.ignore).not.toHaveBeenCalled()
  await screen.unmount()
})

it('keeps cached questions non-actionable after approval until an authoritative refresh succeeds', async () => {
  const host = fakeHost([question()])
  let result!: HumanInteractionControllerView
  const onRender = (value: HumanInteractionControllerView) => {
    result = value
  }
  const screen = await render(<Harness api={host.api} onRender={onRender} />)
  await expect.poll(() => result.activeBatch?.requestId).toBe('question')
  await screen.rerender(<Harness api={host.api} onRender={onRender} hasApproval />)
  host.api.listRequests.mockRejectedValueOnce(new Error('Core unavailable'))
  await screen.rerender(<Harness api={host.api} onRender={onRender} />)
  await expect.poll(() => result.status).toBe('error')
  expect(result.activeBatch).toBeNull()
  expect(result.canInteract).toBe(false)
  expect(result.error).toBe('humanInteraction.error.loadFailed')
  await result.refresh()
  await expect.poll(() => result.activeBatch?.requestId).toBe('question')
  await screen.unmount()
})

it('a previously captured async callback cannot mutate after sync preemption arrives', async () => {
  const host = fakeHost([question()])
  let result!: HumanInteractionControllerView
  const screen = await render(
    <Harness
      api={host.api}
      onRender={(value) => {
        result = value
      }}
    />
  )
  await expect.poll(() => result.activeBatch?.requestId).toBe('question')
  for (const q of question().questions)
    result.setAnswer('question', { kind: 'skipped', questionId: q.id })
  const oldSubmit = result.submit,
    oldIgnore = result.ignore
  host.notify({ ...question('sync', 2), mode: 'sync' })
  await oldSubmit('question')
  await oldIgnore('question')
  expect(host.api.submit).not.toHaveBeenCalled()
  expect(host.api.ignore).not.toHaveBeenCalled()
  await screen.unmount()
})

it('Core reconnect recovers a failed post-approval refresh without requiring window focus', async () => {
  const host = fakeHost([question()])
  let result!: HumanInteractionControllerView
  const onRender = (value: HumanInteractionControllerView) => {
    result = value
  }
  const screen = await render(<Harness api={host.api} onRender={onRender} />)
  await expect.poll(() => result.activeBatch?.requestId).toBe('question')
  await screen.rerender(<Harness api={host.api} onRender={onRender} hasApproval />)
  host.api.listRequests.mockRejectedValueOnce(new Error('Core disconnected'))
  await screen.rerender(<Harness api={host.api} onRender={onRender} />)
  await expect.poll(() => result.status).toBe('error')
  expect(result.canInteract).toBe(false)
  for (const resync of host.resyncListeners) resync()
  await expect.poll(() => result.activeBatch?.requestId).toBe('question')
  expect(result.canInteract).toBe(true)
  await screen.unmount()
})

it('a detached callback cannot submit, ignore or overwrite a draft after a newer async batch takes focus', async () => {
  const host = fakeHost([question('old')])
  let result!: HumanInteractionControllerView
  const screen = await render(
    <Harness
      api={host.api}
      onRender={(value) => {
        result = value
      }}
    />
  )
  await expect.poll(() => result.activeBatch?.requestId).toBe('old')
  for (const q of question('old').questions)
    result.setAnswer('old', { kind: 'skipped', questionId: q.id })
  const old = result
  host.notify(question('new', 2))
  await old.submit('old')
  await old.ignore('old')
  old.setAnswer('old', { kind: 'text', questionId: 'old-two', text: 'late DOM input' })
  expect(host.api.submit).not.toHaveBeenCalled()
  expect(host.api.ignore).not.toHaveBeenCalled()
  await expect.poll(() => result.activeBatch?.requestId).toBe('new')
  result.open('old')
  await expect.poll(() => result.activeBatch?.requestId).toBe('old')
  expect(result.activeDraft.answers['old-two']).toEqual({ kind: 'skipped', questionId: 'old-two' })
  await screen.unmount()
})

it('does not authorize a replacement Host from a late success belonging to the previous Host', async () => {
  const oldHost = fakeHost([question('old')]),
    nextHost = fakeHost([question('next')])
  const pending = deferred<HostInvocationResult<HumanInteractionListOutput>>()
  oldHost.api.listRequests.mockReturnValueOnce(pending.promise)
  nextHost.api.listRequests.mockRejectedValue(new Error('new Host unavailable'))
  let result!: HumanInteractionControllerView
  const onRender = (value: HumanInteractionControllerView) => {
    result = value
  }
  const screen = await render(<Harness api={oldHost.api} onRender={onRender} />)
  await screen.rerender(<Harness api={nextHost.api} onRender={onRender} />)
  await expect.poll(() => result.status).toBe('error')
  pending.resolve({ ok: true, value: { items: [question('old')], nextCursor: null } })
  await expect.poll(() => result.canInteract).toBe(false)
  expect(result.activeBatch).toBeNull()
  await screen.unmount()
})
