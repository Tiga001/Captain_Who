import { randomUUID } from 'node:crypto'

import { describe, expect, it, vi } from 'vitest'
import type {
  ManagedPlaywrightAuthorizationContext,
  ManagedPlaywrightPrepareSensitiveToolInput
} from '@mycopilot/protocol'

import {
  ManagedPlaywrightSensitiveTargetBindingBroker,
  type ManagedPlaywrightPreparedSensitiveTargetBinding,
  type ManagedPlaywrightSensitiveTargetIdentity
} from './ManagedPlaywrightSensitiveTargetBindingBroker'

const NOW = 1_000_000
const TARGET: ManagedPlaywrightSensitiveTargetIdentity = {
  surfaceId: 'main-only-surface',
  generation: 3,
  navigationEpoch: 7,
  origin: 'https://mail.example.test'
}

function prepareInput(
  overrides: Partial<ManagedPlaywrightPrepareSensitiveToolInput> = {}
): ManagedPlaywrightPrepareSensitiveToolInput {
  return {
    bindingRequestId: randomUUID(),
    bindingScope: 'managed_surface',
    runId: 'run-1',
    capabilityId: 'browser_automation',
    activationId: '4af35bbd-cb1e-4b20-a021-92cd6b160829',
    manifestDigest: `sha256:${'a'.repeat(64)}`,
    policyRevision: 9,
    grantExpiresAtMs: NOW + 120_000,
    callId: 'call-1',
    toolName: 'browser_evaluate',
    argumentsDigest: `sha256:${'b'.repeat(64)}`,
    createdAtMs: NOW,
    expiresAtMs: NOW + 60_000,
    filePreparation: null,
    ...overrides
  }
}

function authorization(
  prepared: ManagedPlaywrightPreparedSensitiveTargetBinding,
  input: ManagedPlaywrightPrepareSensitiveToolInput
): ManagedPlaywrightAuthorizationContext {
  return {
    runId: input.runId,
    capabilityId: 'browser_automation',
    activationId: input.activationId,
    manifestDigest: input.manifestDigest,
    policyRevision: input.policyRevision,
    grantExpiresAtMs: input.grantExpiresAtMs,
    invocationId: randomUUID(),
    callId: input.callId,
    triggerToolName: input.toolName,
    callReason: 'Evaluate the approved fixture document.',
    builtinToolGrant: {
      grantId: randomUUID(),
      approvalId: randomUUID(),
      argumentsDigest: input.argumentsDigest,
      resourceScopeDigest: `sha256:${'c'.repeat(64)}`,
      targetBindingId: prepared.bindingId,
      targetBindingDigest: prepared.targetBindingDigest,
      origin: prepared.origin,
      riskKinds: ['page_script_execution'],
      expiresAtMs: input.expiresAtMs
    }
  }
}

function harness(
  options: {
    maxBindings?: number
    releasePreparedFiles?: (owner: { runId: string; callId: string }) => void
  } = {}
) {
  let now = NOW
  let target: ManagedPlaywrightSensitiveTargetIdentity | null = { ...TARGET }
  let blockAtFinish = false
  const finishFence = vi.fn(() => {
    if (blockAtFinish) throw new Error('navigation blocked')
  })
  const beginDispatchFence = vi.fn(() => ({ finish: finishFence }))
  const broker = new ManagedPlaywrightSensitiveTargetBindingBroker({
    beginDispatchFence,
    getActiveTarget: () => target,
    maxBindings: options.maxBindings,
    now: () => now,
    releasePreparedFiles: options.releasePreparedFiles,
    secret: new Uint8Array(32).fill(7)
  })
  return {
    beginDispatchFence,
    broker,
    finishFence,
    setBlockAtFinish: (value: boolean) => {
      blockAtFinish = value
    },
    setNow: (value: number) => {
      now = value
    },
    setTarget: (value: ManagedPlaywrightSensitiveTargetIdentity | null) => {
      target = value
    }
  }
}

describe('ManagedPlaywrightSensitiveTargetBindingBroker', () => {
  it('freezes an opaque exact document binding and consumes it once at dispatch', () => {
    const fixture = harness()
    const input = prepareInput()
    const prepared = fixture.broker.prepare(input)

    expect(prepared).toMatchObject({
      origin: TARGET.origin,
      createdAtMs: input.createdAtMs,
      expiresAtMs: input.expiresAtMs
    })
    expect(JSON.stringify(prepared)).not.toMatch(/surface|generation|navigationEpoch/)
    expect(fixture.broker.prepare(input)).toEqual(prepared)

    const lease = fixture.broker.acquire(authorization(prepared, input))
    expect(lease.target).toEqual(TARGET)
    lease.markDispatched()
    expect(fixture.beginDispatchFence).toHaveBeenCalledWith(TARGET)
    expect(fixture.broker.snapshot()).toEqual({ bindings: 0, requests: 0 })
    lease.finish()
    expect(fixture.finishFence).toHaveBeenCalledOnce()
    expect(() => fixture.broker.acquire(authorization(prepared, input))).toThrow(
      expect.objectContaining({ code: 'reused' })
    )
  })

  it('binds profile-scoped cookie/storage authority without an active surface or navigation fence', () => {
    const fixture = harness()
    fixture.setTarget(null)
    const input = prepareInput({
      bindingScope: 'managed_browser_profile',
      toolName: 'browser_cookie_list',
      callId: 'call-profile',
      argumentsDigest: `sha256:${'e'.repeat(64)}`
    })
    const prepared = fixture.broker.prepare(input)

    expect(prepared.origin).toBeNull()
    expect(JSON.stringify(prepared)).not.toMatch(/surface|generation|navigationEpoch/)
    const lease = fixture.broker.acquire(authorization(prepared, input))
    expect(lease.target).toBeUndefined()
    lease.markDispatched()
    lease.finish()
    expect(fixture.beginDispatchFence).not.toHaveBeenCalled()
    expect(fixture.broker.snapshot()).toEqual({ bindings: 0, requests: 0 })
  })

  it('projects only safe file identity and releases proposal files unless dispatch consumed them', () => {
    const releasePreparedFiles = vi.fn()
    const fixture = harness({ releasePreparedFiles })
    const input = prepareInput({
      filePreparation: { mode: 'resolved_paths', paths: ['/process-only/workspace/deck.pptx'] }
    })
    const fileAuthority = {
      handles: ['browser-file:123e4567-e89b-42d3-a456-426614174000'],
      basenames: ['deck.pptx'],
      fileRevisionDigest: `sha256:${'d'.repeat(64)}`
    }
    const prepared = fixture.broker.prepare(input, fileAuthority)

    expect(prepared).toMatchObject({
      fileBasenames: ['deck.pptx'],
      fileRevisionDigest: fileAuthority.fileRevisionDigest
    })
    expect(JSON.stringify(prepared)).not.toContain('/process-only')
    expect(JSON.stringify(prepared)).not.toContain('browser-file:')
    const lease = fixture.broker.acquire(authorization(prepared, input))
    expect(lease.preparedFileHandles).toEqual(fileAuthority.handles)
    lease.finish()
    expect(releasePreparedFiles).toHaveBeenCalledExactlyOnceWith({
      runId: input.runId,
      callId: input.callId
    })

    releasePreparedFiles.mockClear()
    const dispatchedInput = prepareInput({
      bindingRequestId: randomUUID(),
      callId: 'call-dispatched',
      filePreparation: { mode: 'resolved_paths', paths: ['/process-only/workspace/deck-2.pptx'] }
    })
    const dispatchedAuthority = {
      ...fileAuthority,
      handles: ['browser-file:223e4567-e89b-42d3-a456-426614174000']
    }
    const dispatched = fixture.broker.prepare(dispatchedInput, dispatchedAuthority)
    const dispatchedLease = fixture.broker.acquire(authorization(dispatched, dispatchedInput))
    dispatchedLease.markDispatched()
    dispatchedLease.finish()
    expect(releasePreparedFiles).not.toHaveBeenCalled()
  })

  it('invalidates a duplicate preparation whose opaque file authority drifted', () => {
    const releasePreparedFiles = vi.fn()
    const fixture = harness({ releasePreparedFiles })
    const input = prepareInput({
      filePreparation: { mode: 'resolved_paths', paths: ['/process-only/workspace/deck.pptx'] }
    })
    const first = {
      handles: ['browser-file:123e4567-e89b-42d3-a456-426614174000'],
      basenames: ['deck.pptx'],
      fileRevisionDigest: `sha256:${'d'.repeat(64)}`
    }
    fixture.broker.prepare(input, first)

    expect(() =>
      fixture.broker.prepare(input, {
        ...first,
        handles: ['browser-file:223e4567-e89b-42d3-a456-426614174000']
      })
    ).toThrow(expect.objectContaining({ code: 'drifted' }))
    expect(fixture.broker.snapshot()).toEqual({ bindings: 0, requests: 0 })
    expect(releasePreparedFiles).toHaveBeenCalledExactlyOnceWith({
      runId: input.runId,
      callId: input.callId
    })
  })

  it.each([
    ['same-origin tab', { ...TARGET, surfaceId: 'other-surface' }],
    ['generation', { ...TARGET, generation: TARGET.generation + 1 }],
    ['same-origin document epoch', { ...TARGET, navigationEpoch: TARGET.navigationEpoch + 1 }],
    ['origin', { ...TARGET, origin: 'https://other.example.test' }]
  ])('fails closed after %s drift and deletes proposal authority', (_name, drifted) => {
    const fixture = harness()
    const input = prepareInput()
    const prepared = fixture.broker.prepare(input)
    fixture.setTarget(drifted)
    expect(() => fixture.broker.acquire(authorization(prepared, input))).toThrow(
      expect.objectContaining({ code: 'origin_drifted' })
    )
    expect(fixture.broker.snapshot()).toEqual({ bindings: 0, requests: 0 })
    expect(fixture.beginDispatchFence).not.toHaveBeenCalled()
  })

  it('rejects concurrent/double use and releases an acquired pre-dispatch lease', () => {
    const fixture = harness()
    const input = prepareInput()
    const prepared = fixture.broker.prepare(input)
    const auth = authorization(prepared, input)
    const lease = fixture.broker.acquire(auth)
    expect(() => fixture.broker.acquire(auth)).toThrow(expect.objectContaining({ code: 'reused' }))
    expect(fixture.broker.releaseToolCall({ runId: input.runId, callId: input.callId })).toBe(1)
    expect(fixture.broker.snapshot()).toEqual({ bindings: 0, requests: 0 })
    expect(() => lease.markDispatched()).toThrow(expect.objectContaining({ code: 'reused' }))
  })

  it('bounds capacity, expires lazily, and supports cancellation by request id', () => {
    const fixture = harness({ maxBindings: 1 })
    const first = prepareInput()
    fixture.broker.prepare(first)
    expect(() => fixture.broker.prepare(prepareInput({ callId: 'call-2' }))).toThrow(
      expect.objectContaining({ code: 'busy' })
    )
    expect(fixture.broker.releaseByRequestId(first.bindingRequestId)).toBe(true)
    expect(fixture.broker.snapshot()).toEqual({ bindings: 0, requests: 0 })

    fixture.broker.prepare(prepareInput({ callId: 'call-expiring' }))
    fixture.setNow(NOW + 60_001)
    expect(fixture.broker.snapshot()).toEqual({ bindings: 0, requests: 0 })
  })

  it('fails closed when the dispatch fence detects navigation and releases it on run revoke', () => {
    const fixture = harness()
    const firstInput = prepareInput()
    const firstPrepared = fixture.broker.prepare(firstInput)
    const firstLease = fixture.broker.acquire(authorization(firstPrepared, firstInput))
    fixture.setBlockAtFinish(true)
    firstLease.markDispatched()
    expect(() => firstLease.finish()).toThrow(expect.objectContaining({ code: 'origin_drifted' }))

    fixture.setBlockAtFinish(false)
    const secondInput = prepareInput({ callId: 'call-2' })
    const secondPrepared = fixture.broker.prepare(secondInput)
    const secondLease = fixture.broker.acquire(authorization(secondPrepared, secondInput))
    secondLease.markDispatched()
    expect(fixture.broker.releaseRun(secondInput.runId)).toBe(1)
    expect(fixture.finishFence).toHaveBeenCalledTimes(2)
    expect(fixture.broker.snapshot()).toEqual({ bindings: 0, requests: 0 })
  })

  it('closes the fence if the document epoch changes during lock installation', () => {
    let target: ManagedPlaywrightSensitiveTargetIdentity = { ...TARGET }
    const finish = vi.fn()
    const broker = new ManagedPlaywrightSensitiveTargetBindingBroker({
      beginDispatchFence: () => {
        target = { ...target, navigationEpoch: target.navigationEpoch + 1 }
        return { finish }
      },
      getActiveTarget: () => target,
      now: () => NOW,
      secret: new Uint8Array(32).fill(9)
    })
    const input = prepareInput()
    const prepared = broker.prepare(input)
    const lease = broker.acquire(authorization(prepared, input))
    expect(() => lease.markDispatched()).toThrow(
      expect.objectContaining({ code: 'origin_drifted' })
    )
    expect(finish).toHaveBeenCalledOnce()
    lease.finish()
    expect(broker.snapshot()).toEqual({ bindings: 0, requests: 0 })
  })
})
