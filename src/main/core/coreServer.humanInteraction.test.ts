import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { beforeEach, describe, expect, it, vi } from 'vitest'
const { request, onNotification } = vi.hoisted(() => ({
  request: vi.fn(),
  onNotification: vi.fn()
}))
vi.mock('./jsonRpcClient', () => ({
  CoreJsonRpcClient: class {
    readonly request = request
    readonly onNotification = onNotification
  }
}))
import { CoreServer } from './coreServer'

const fixture = JSON.parse(
  readFileSync(
    resolve(process.cwd(), 'packages/protocol/fixtures/human-interaction-v1.json'),
    'utf8'
  )
)
function responseSnapshot(kind: 'submitted' | 'ignored') {
  return {
    ...fixture.request,
    status: kind,
    revision: 1,
    updatedAt: 20,
    response: {
      responseId: 'response-1',
      requestId: fixture.request.requestId,
      submissionId:
        kind === 'submitted' ? fixture.submit.submissionId : fixture.ignore.submissionId,
      kind,
      answers: kind === 'submitted' ? fixture.submit.answers : [],
      createdAt: 20
    },
    delivery:
      kind === 'submitted'
        ? {
            responseId: 'response-1',
            status: 'pending',
            revision: 0,
            targetRunId: null,
            userMessageId: null,
            errorCode: null
          }
        : null
  }
}

beforeEach(() => {
  request.mockReset()
  onNotification.mockReset().mockReturnValue(vi.fn())
})

describe('Core human interaction transport', () => {
  it('routes all five RPCs with strict input/output contracts and preserves independent delivery', async () => {
    const server = new CoreServer()
    request
      .mockResolvedValueOnce(fixture.settings)
      .mockResolvedValueOnce(fixture.settingsUpdated)
      .mockResolvedValueOnce({ items: [fixture.request], nextCursor: null })
      .mockResolvedValueOnce(responseSnapshot('submitted'))
      .mockResolvedValueOnce(responseSnapshot('ignored'))
    await expect(server.getHumanInteractionSettings({})).resolves.toEqual(fixture.settings)
    await expect(server.updateHumanInteractionSettings(fixture.settingsUpdate)).resolves.toEqual(
      fixture.settingsUpdated
    )
    await expect(server.listHumanInteractionRequests(fixture.listInput)).resolves.toEqual({
      items: [fixture.request],
      nextCursor: null
    })
    await expect(server.submitHumanInteractionRequest(fixture.submit)).resolves.toEqual(
      responseSnapshot('submitted')
    )
    await expect(server.ignoreHumanInteractionRequest(fixture.ignore)).resolves.toEqual(
      responseSnapshot('ignored')
    )
    expect(request.mock.calls).toEqual([
      ['humanInteraction.getSettings', {}],
      ['humanInteraction.updateSettings', fixture.settingsUpdate],
      ['humanInteraction.listRequests', fixture.listInput],
      ['humanInteraction.submit', fixture.submit],
      ['humanInteraction.ignore', fixture.ignore]
    ])
  })

  it('rejects invalid input before RPC and does not fabricate settings when Core fails', async () => {
    const server = new CoreServer()
    await expect(
      server.updateHumanInteractionSettings({
        ...fixture.settingsUpdate,
        expectedRevision: Number.MAX_SAFE_INTEGER + 1
      })
    ).rejects.toThrow()
    await expect(
      server.ignoreHumanInteractionRequest({ ...fixture.ignore, answers: [] })
    ).rejects.toThrow()
    expect(request).not.toHaveBeenCalled()
    request.mockRejectedValue(new Error('Core unavailable'))
    await expect(server.getHumanInteractionSettings({})).rejects.toThrow('Core unavailable')
  })

  it('preserves asynchronous delivery routing independently of a submitted question', async () => {
    const server = new CoreServer()
    const changed = vi.fn()
    server.onHumanInteractionRequestChanged(changed)
    const submitted = { ...responseSnapshot('submitted'), mode: 'async' }
    const deliveries = [
      submitted.delivery,
      { ...submitted.delivery, status: 'bound', revision: 1, targetRunId: 'active-run' },
      { ...submitted.delivery, status: 'applied', revision: 2, targetRunId: 'active-run' },
      {
        ...submitted.delivery,
        status: 'applied',
        revision: 2,
        targetRunId: 'later-human-root-run',
        userMessageId: 'human-response-message'
      },
      { ...submitted.delivery, status: 'cancelled', revision: 1, errorCode: 'run_stopped' }
    ]
    for (const delivery of deliveries) {
      const snapshot = { ...submitted, delivery }
      request.mockResolvedValueOnce(snapshot)
      await expect(server.submitHumanInteractionRequest(fixture.submit)).resolves.toEqual(snapshot)
      onNotification.mock.calls[0][1](snapshot)
      expect(changed).toHaveBeenLastCalledWith(snapshot)
    }
    expect(request.mock.calls.every(([method]) => method === 'humanInteraction.submit')).toBe(true)
    expect(changed).toHaveBeenCalledTimes(deliveries.length)
  })

  it('rejects a cross-conversation page and a response for a different submission', async () => {
    const server = new CoreServer()
    request.mockResolvedValue({
      items: [{ ...fixture.request, conversationId: 'foreign' }],
      nextCursor: null
    })
    await expect(server.listHumanInteractionRequests(fixture.listInput)).rejects.toThrow(
      'ownership'
    )
    const response = responseSnapshot('submitted')
    request.mockResolvedValue({
      ...response,
      response: { ...response.response, submissionId: 'foreign' }
    })
    await expect(server.submitHumanInteractionRequest(fixture.submit)).rejects.toThrow('identity')
    request.mockResolvedValue({ ...fixture.settings, rawPolicy: 'private' })
    await expect(server.getHumanInteractionSettings({})).rejects.toThrow()
  })

  it('validates both notifications and returns their unsubscribe handles', () => {
    const server = new CoreServer()
    const settings = vi.fn(),
      changed = vi.fn()
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const stopSettings = server.onHumanInteractionSettingsChanged(settings)
    const stopRequests = server.onHumanInteractionRequestChanged(changed)
    expect(onNotification.mock.calls.map(([name]) => name)).toEqual([
      'humanInteraction.settingsChanged',
      'humanInteraction.requestChanged'
    ])
    onNotification.mock.calls[0][1](fixture.settings)
    onNotification.mock.calls[0][1]({ ...fixture.settings, privatePolicy: 'secret' })
    onNotification.mock.calls[1][1](fixture.request)
    onNotification.mock.calls[1][1]({ ...fixture.request, delivery: undefined })
    expect(settings).toHaveBeenCalledExactlyOnceWith(fixture.settings)
    expect(changed).toHaveBeenCalledExactlyOnceWith(fixture.request)
    expect(stopSettings).toBe(onNotification.mock.results[0].value)
    expect(stopRequests).toBe(onNotification.mock.results[1].value)
    warn.mockRestore()
  })
})
