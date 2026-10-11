import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import {
  parsePersistedAgentRunJson,
  stringifyPersistedAgentRun,
  STORED_AGENT_RUN_CORRUPTION_ERROR
} from '../../storage/persistedAgentRun'

type RejectedMutation = {
  name: string
  remove: string[]
  set: Record<string, unknown>
}

type PersistedAgentRunProjectionFixture = {
  schemaVersion: number
  input: Record<string, unknown>
  expectedCanonical: Record<string, unknown>
  rejectedMutations: RejectedMutation[]
}

const fixture = JSON.parse(
  readFileSync(
    resolve(process.cwd(), 'packages/protocol/fixtures/persisted-agent-run-projection-v1.json'),
    'utf8'
  )
) as PersistedAgentRunProjectionFixture

function applyRejectedMutation(mutation: RejectedMutation): Record<string, unknown> {
  const candidate = structuredClone(fixture.input)
  for (const key of mutation.remove) delete candidate[key]
  Object.assign(candidate, structuredClone(mutation.set))
  return candidate
}

describe('persisted Agent run Rust-to-Renderer projection contract', () => {
  it('preserves an opaque Core checkpoint through parsing and safe serialization', () => {
    const coreCheckpoint = {
      generation: '01a12a4f-5137-7e90-b173-44145df4714e',
      sequence: 0,
      fingerprint: 'a'.repeat(64)
    }
    const projection = { ...fixture.expectedCanonical, coreCheckpoint }
    const parsed = parsePersistedAgentRunJson(JSON.stringify(projection))
    expect(parsed?.coreCheckpoint).toEqual(coreCheckpoint)
    expect(JSON.parse(stringifyPersistedAgentRun(parsed)!)).toHaveProperty(
      'coreCheckpoint',
      coreCheckpoint
    )
    expect(parsePersistedAgentRunJson(JSON.stringify(fixture.expectedCanonical))).toBeDefined()
  })

  it.each([
    { generation: 'invalid', sequence: 0, fingerprint: 'a'.repeat(64) },
    {
      generation: '01a12a4f-5137-7e90-b173-44145df4714e',
      sequence: -1,
      fingerprint: 'a'.repeat(64)
    },
    {
      generation: '01a12a4f-5137-7e90-b173-44145df4714e',
      sequence: 0.5,
      fingerprint: 'a'.repeat(64)
    },
    {
      generation: '01a12a4f-5137-7e90-b173-44145df4714e',
      sequence: 0,
      fingerprint: 'a'.repeat(63)
    },
    {
      generation: '01a12a4f-5137-7e90-b173-44145df4714e',
      sequence: 0,
      fingerprint: 'g'.repeat(64)
    },
    {
      generation: '01a12a4f-5137-7e90-b173-44145df4714e',
      sequence: 0,
      fingerprint: 'a'.repeat(64),
      extra: true
    }
  ])('rejects malformed Core checkpoint %#', (coreCheckpoint) => {
    expect(() =>
      parsePersistedAgentRunJson(JSON.stringify({ ...fixture.expectedCanonical, coreCheckpoint }))
    ).toThrow(STORED_AGENT_RUN_CORRUPTION_ERROR)
  })

  it.each(['output_limit_reached', 'empty_response', 'stream_interrupted'])(
    'preserves durable interruption %s',
    (reason) => {
      const projection = structuredClone(fixture.expectedCanonical)
      projection.interruption = { reason }
      expect(parsePersistedAgentRunJson(JSON.stringify(projection))?.interruption).toEqual({
        reason
      })
    }
  )
  it('preserves the uncertain-admission marker when reloading a saved local attempt', () => {
    const projection = structuredClone(fixture.expectedCanonical)
    projection.interruption = { reason: 'admission_unconfirmed' }
    expect(parsePersistedAgentRunJson(JSON.stringify(projection))?.interruption).toEqual({
      reason: 'admission_unconfirmed'
    })
  })
  it('retains the explicit user-stop fact when history is reloaded', () => {
    const projection = { ...fixture.expectedCanonical, status: 'cancelled', userInterrupted: true }
    expect(parsePersistedAgentRunJson(JSON.stringify(projection))?.userInterrupted).toBe(true)
    expect(() =>
      parsePersistedAgentRunJson(JSON.stringify({ ...projection, userInterrupted: 'true' }))
    ).toThrow(STORED_AGENT_RUN_CORRUPTION_ERROR)
  })

  it('parses the exact canonical projection produced by Rust', () => {
    expect(fixture.schemaVersion).toBe(1)

    const parsed = parsePersistedAgentRunJson(JSON.stringify(fixture.expectedCanonical))

    expect(parsed).toBeDefined()
    expect(parsed?.fileChangeProposals).toEqual(fixture.expectedCanonical.fileChangeProposals)
    expect(parsed?.fileChanges).toEqual(fixture.expectedCanonical.fileChanges)
    expect(parsed?.messageStreamCheckpoints).toEqual({
      'stream-contract': { previousContent: 'partial response' }
    })
    expect(parsed?.timeline).toContainEqual({
      id: 'trace-message-contract',
      type: 'message',
      content: 'current traced narration',
      traceSequence: 0
    })
    expect(parsed?.interruption).toEqual({ reason: 'service_connection_failed' })
    expect(fixture.expectedCanonical).not.toHaveProperty('diffs')
    expect(fixture.expectedCanonical).not.toHaveProperty('fileDrafts')
  })

  it.each(fixture.rejectedMutations)('rejects $name instead of repairing it', (mutation) => {
    expect(() =>
      parsePersistedAgentRunJson(JSON.stringify(applyRejectedMutation(mutation)))
    ).toThrow(STORED_AGENT_RUN_CORRUPTION_ERROR)
  })
})
