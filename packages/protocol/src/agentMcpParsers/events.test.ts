import { describe, expect, it } from 'vitest'
import { parseAgentEventForHost } from '../index'
import { createAgentMcpFixtures } from './fixtures'

const { callId } = createAgentMcpFixtures()

describe('durable presentation trace ordering contract', () => {
  it('accepts only the exact built-in capability Tool identity projection', () => {
    const event = {
      type: 'tool_call',
      runId: 'run-browser-capability',
      traceSequence: 7,
      identity: {
        type: 'builtin_capability',
        capabilityId: 'browser_automation',
        managedMcpId: 'builtin.browser_automation.mcp',
        packageName: '@playwright/mcp',
        packageVersion: '0.0.79',
        upstreamCatalogDigest: `sha256:${'1'.repeat(64)}`,
        policyDigest: `sha256:${'2'.repeat(64)}`,
        manifestDigest: `sha256:${'e'.repeat(64)}`,
        toolId: 'browser.navigate',
        rawName: 'browser_navigate',
        modelName: 'browser_navigate',
        upstreamSchemaDigest: `sha256:${'3'.repeat(64)}`,
        hostOverlayDigest: `sha256:${'4'.repeat(64)}`,
        hostInputSchemaDigest: `sha256:${'5'.repeat(64)}`
      },
      call: {
        id: callId,
        tool: 'browser_navigate',
        args: { url: 'https://user:PRIVATE_PASSWORD@example.com/' },
        approvalStatus: 'not_required',
        reason: null
      }
    }

    expect(parseAgentEventForHost(event)).toEqual({
      ...event,
      call: { ...event.call, args: {} }
    })
    expect(JSON.stringify(parseAgentEventForHost(event))).not.toContain('PRIVATE_PASSWORD')
    expect(() =>
      parseAgentEventForHost({
        ...event,
        identity: { ...event.identity, cdpEndpoint: 'PRIVATE_CDP_CANARY' }
      })
    ).toThrow(/unexpected field/)
    expect(() =>
      parseAgentEventForHost({
        ...event,
        call: { ...event.call, tool: 'browser_snapshot' }
      })
    ).toThrow(/identity must match call\.tool/)
  })

  it('strictly parses required Tool, narration, compaction, and Runtime error sequence fields', () => {
    const toolCall = {
      type: 'tool_call',
      runId: 'run-trace',
      traceSequence: 4,
      identity: { type: 'builtin', toolName: 'read_file' },
      call: {
        id: callId,
        tool: 'read_file',
        args: { path: 'README.md' },
        approvalStatus: 'not_required',
        reason: null
      }
    }
    expect(parseAgentEventForHost(toolCall)).toEqual(toolCall)
    const missingIdentity = { ...toolCall } as Record<string, unknown>
    delete missingIdentity.identity
    expect(() => parseAgentEventForHost(missingIdentity)).toThrow(/identity/)
    expect(() =>
      parseAgentEventForHost({
        ...toolCall,
        identity: { type: 'builtin', toolName: 'different_tool' }
      })
    ).toThrow(/identity must match call\.tool/)
    expect(
      parseAgentEventForHost({
        type: 'message_stream_committed',
        runId: 'run-trace',
        streamId: 'stream-trace',
        traceSequence: null
      })
    ).toMatchObject({ type: 'message_stream_committed', traceSequence: null })
    expect(
      parseAgentEventForHost({
        type: 'context_compaction_started',
        runId: 'run-trace',
        operationId: 'compact-1',
        traceSequence: 5
      })
    ).toMatchObject({ type: 'context_compaction_started', traceSequence: 5 })
    expect(
      parseAgentEventForHost({
        type: 'context_compaction_finished',
        runId: 'run-trace',
        operationId: 'compact-1',
        outcome: 'applied',
        traceSequence: 5
      })
    ).toMatchObject({ type: 'context_compaction_finished', traceSequence: 5 })
    expect(
      parseAgentEventForHost({
        type: 'error',
        runId: 'run-trace',
        traceSequence: 6,
        message: 'stopped',
        recoverable: false
      })
    ).toMatchObject({ type: 'error', traceSequence: 6 })

    for (const event of [
      toolCall,
      {
        type: 'message_stream_committed',
        runId: 'run-trace',
        streamId: 'stream-trace',
        traceSequence: 3
      },
      {
        type: 'context_compaction_started',
        runId: 'run-trace',
        operationId: 'compact-1',
        traceSequence: 5
      },
      {
        type: 'context_compaction_finished',
        runId: 'run-trace',
        operationId: 'compact-1',
        outcome: 'applied',
        traceSequence: 5
      },
      {
        type: 'error',
        runId: 'run-trace',
        traceSequence: null,
        message: 'stopped',
        recoverable: false
      }
    ]) {
      const missing = { ...event } as Record<string, unknown>
      delete missing.traceSequence
      expect(() => parseAgentEventForHost(missing)).toThrow(/traceSequence/)
    }
  })
})

describe('current LLM retry Host contract', () => {
  it('keeps the complete structured retry metadata', () => {
    const parsed = parseAgentEventForHost({
      type: 'llm_retry',
      runId: 'run-retry',
      streamId: 'stream-1',
      category: 'rate_limited',
      providerCode: 'rate_limit_exceeded',
      delayMs: 5_000,
      retryAt: 1_800_000_005_000,
      attempt: 2,
      maxAttempts: 3
    })

    expect(parsed).toEqual({
      type: 'llm_retry',
      runId: 'run-retry',
      streamId: 'stream-1',
      category: 'rate_limited',
      providerCode: 'rate_limit_exceeded',
      delayMs: 5_000,
      retryAt: 1_800_000_005_000,
      attempt: 2,
      maxAttempts: 3
    })
  })

  it('rejects the removed reason shape, missing scheduling fields, and unknown categories', () => {
    const current = {
      type: 'llm_retry',
      runId: 'run-retry',
      streamId: 'stream-current',
      category: 'network',
      delayMs: 1_000,
      retryAt: 1_800_000_001_000,
      attempt: 2,
      maxAttempts: 3
    }
    expect(() =>
      parseAgentEventForHost({ ...current, reason: 'removed provider-authored text' })
    ).toThrow(/unexpected field reason/)
    expect(() => {
      const missingDelay: Record<string, unknown> = { ...current }
      delete missingDelay.delayMs
      parseAgentEventForHost(missingDelay)
    }).toThrow(/delayMs/)
    expect(() =>
      parseAgentEventForHost({
        ...current,
        type: 'llm_retry',
        streamId: 'stream-future',
        category: 'new_provider_category'
      })
    ).toThrow(/supported retry category/)
  })

  it('replaces stream reset diagnostics with a stable lifecycle reason', () => {
    expect(
      parseAgentEventForHost({
        type: 'message_stream_reset',
        runId: 'run-retry',
        streamId: 'stream-1',
        reason: 'secret upstream response body'
      })
    ).toEqual({
      type: 'message_stream_reset',
      runId: 'run-retry',
      streamId: 'stream-1',
      reason: 'retrying_model_request'
    })
  })

  it('bounds retry scheduling metadata and requires canonical provider codes', () => {
    const base = {
      type: 'llm_retry',
      runId: 'run-retry',
      streamId: 'stream-1',
      category: 'network',
      delayMs: 1_000,
      retryAt: 1_800_000_001_000,
      attempt: 2,
      maxAttempts: 6
    }
    expect(() => parseAgentEventForHost({ ...base, delayMs: 60_001 })).toThrow(/delayMs/)
    expect(() => parseAgentEventForHost({ ...base, maxAttempts: 7 })).toThrow(/must not exceed 6/)
    expect(() =>
      parseAgentEventForHost({ ...base, providerCode: 'Provider response body' })
    ).toThrow(/machine-readable code/)
  })
})
