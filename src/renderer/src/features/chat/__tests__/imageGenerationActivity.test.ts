// Renderer regressions for safe image-generation status and Artifact derivation.
import type {
  AgentImageGenerationResult,
  AgentToolCall,
  AgentToolResult
} from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import type { ChatAgentRunView } from '../chatTypes'
import {
  getImageGenerationActivityView,
  getImageGenerationArtifactEntries
} from '../../imageGeneration/imageGenerationActivity'

const HASH = 'a'.repeat(64)

function call(overrides: Partial<AgentToolCall> = {}): AgentToolCall {
  return {
    id: 'image-call-1',
    tool: 'image_generation',
    args: { request: { operation: 'generate', hasInputImage: false }, reason: 'Create a cover' },
    approvalStatus: 'not_required',
    reason: null,
    ...overrides
  }
}

function parsedResult(status: AgentImageGenerationResult['status']): AgentImageGenerationResult {
  const base = {
    schemaVersion: 1 as const,
    operation: 'generate' as const,
    audit: {
      executionId: 'execution-1',
      requestFingerprint: `sha256:${HASH}`,
      providerProfileId: 'profile-1',
      adapterId: 'smartmlSeedream',
      profileRevision: 1,
      modelId: 'image-model',
      createdAt: 100,
      completedAt: 120,
      durationMs: 20
    }
  }

  if (status === 'succeeded') {
    return {
      ...base,
      status,
      artifact: {
        artifactId: `sha256:${HASH}`,
        uri: `image-artifact://sha256/${HASH}`,
        kind: 'image',
        format: 'png',
        mimeType: 'image/png',
        width: 1024,
        height: 1024,
        sizeBytes: 2048,
        sha256: HASH
      }
    }
  }

  return {
    ...base,
    status,
    failure: {
      code: status === 'cancelled' ? 'cancelled' : 'providerFailed',
      phase: 'provider',
      message: 'The image service could not finish this request.',
      recovery: 'Check the image configuration and try again.',
      retryable: status === 'failed',
      generationMayHaveSucceeded: status === 'outcomeIndeterminate',
      providerSucceeded: status === 'commitIndeterminate',
      artifactCommitMayHaveSucceeded: status === 'commitIndeterminate'
    }
  }
}

function result(value: unknown, overrides: Partial<AgentToolResult> = {}): AgentToolResult {
  return {
    callId: 'image-call-1',
    tool: 'image_generation',
    ok: true,
    result: value,
    ...overrides
  }
}

function run(overrides: Partial<ChatAgentRunView> = {}): ChatAgentRunView {
  return {
    runId: 'run-1',
    status: 'completed',
    toolDefinitions: [],
    toolCalls: [],
    toolResults: [],
    approvals: [],
    diffs: [],
    timeline: [],
    ...overrides
  }
}

describe('image generation activity projection', () => {
  it('keeps a call in one card while it moves from running to succeeded', () => {
    expect(getImageGenerationActivityView(call(), undefined)).toMatchObject({
      operation: 'generate',
      reason: 'Create a cover',
      status: 'running'
    })
    expect(getImageGenerationActivityView(call(), result(parsedResult('succeeded')))).toMatchObject(
      { operation: 'generate', status: 'completed' }
    )
  })

  it.each(['failed', 'cancelled', 'outcomeIndeterminate', 'commitIndeterminate'] as const)(
    'preserves the %s terminal contract and safe recovery text',
    (status) => {
      expect(getImageGenerationActivityView(call(), result(parsedResult(status)))).toMatchObject({
        status,
        failureMessage: 'The image service could not finish this request.',
        failureRecovery: 'Check the image configuration and try again.'
      })
    }
  )

  it('degrades preflight and malformed results without exposing raw fields', () => {
    const view = getImageGenerationActivityView(
      call({
        args: {
          request: { operation: 'edit', hasInputImage: true, inputPath: '/private/input.png' },
          reason: null
        }
      }),
      result({ providerUrl: 'https://private.example', apiKey: 'secret' }, { ok: false })
    )

    expect(view).toEqual({ operation: 'edit', status: 'failed' })
    expect(JSON.stringify(view)).not.toContain('private.example')
    expect(JSON.stringify(view)).not.toContain('secret')
    expect(JSON.stringify(view)).not.toContain('/private/input.png')
  })

  it('creates cards only from ok succeeded results and deduplicates artifact ids', () => {
    const firstCall = call()
    const duplicateCall = call({ id: 'image-call-2' })
    const failedCall = call({ id: 'image-call-3' })
    const succeeded = parsedResult('succeeded')
    const currentRun = run({
      toolCalls: [firstCall, duplicateCall, failedCall],
      toolResults: [
        result(succeeded),
        result(succeeded, { callId: duplicateCall.id }),
        result(succeeded, { callId: failedCall.id, ok: false })
      ]
    })

    expect(getImageGenerationArtifactEntries(currentRun)).toEqual([
      {
        artifact: succeeded.status === 'succeeded' ? succeeded.artifact : undefined,
        callId: firstCall.id,
        operation: 'generate'
      }
    ])
  })

  it('does not create cards for any unsuccessful terminal state', () => {
    const statuses = ['failed', 'cancelled', 'outcomeIndeterminate', 'commitIndeterminate'] as const
    const currentRun = run({
      toolCalls: statuses.map((_, index) => call({ id: `image-call-${index}` })),
      toolResults: statuses.map((status, index) =>
        result(parsedResult(status), { callId: `image-call-${index}` })
      )
    })

    expect(getImageGenerationArtifactEntries(currentRun)).toEqual([])
  })

  it('derives identical activity and Artifact cards after JSON history restoration', () => {
    const imageCall = call()
    const imageResult = result(parsedResult('succeeded'))
    const liveRun = run({ toolCalls: [imageCall], toolResults: [imageResult] })
    const restoredRun = JSON.parse(JSON.stringify(liveRun)) as ChatAgentRunView

    expect(
      getImageGenerationActivityView(restoredRun.toolCalls[0], restoredRun.toolResults[0])
    ).toEqual(getImageGenerationActivityView(imageCall, imageResult))
    expect(getImageGenerationArtifactEntries(restoredRun)).toEqual(
      getImageGenerationArtifactEntries(liveRun)
    )
  })

  it('adapts authoritative run_command and command_session image receipts to existing cards', () => {
    const commandCall = call({ id: 'command-1', tool: 'run_command', args: {} })
    const sessionCall = call({ id: 'session-1', tool: 'command_session', args: {} })
    const output = {
      name: 'pages/page-307.png',
      kind: 'image',
      readPath: `image-artifact://sha256/${HASH}`,
      mimeType: 'image/png',
      sizeBytes: 2048,
      sha256: HASH,
      width: 1200,
      height: 1600
    }
    const currentRun = run({
      toolCalls: [commandCall, sessionCall],
      toolResults: [
        result(
          { execution: { exitCode: 0, outputs: [output] } },
          {
            callId: commandCall.id,
            tool: commandCall.tool
          }
        ),
        result(
          { status: 'completed', outputs: [output] },
          {
            callId: sessionCall.id,
            tool: sessionCall.tool
          }
        )
      ]
    })

    expect(getImageGenerationArtifactEntries(currentRun)).toEqual([
      {
        artifact: {
          artifactId: `sha256:${HASH}`,
          uri: output.readPath,
          kind: 'image',
          format: 'png',
          mimeType: 'image/png',
          width: 1200,
          height: 1600,
          sizeBytes: 2048,
          sha256: HASH
        },
        callId: commandCall.id,
        displayName: 'pages/page-307.png',
        operation: 'command'
      }
    ])
  })

  it('restores a managed image card from the original run_command Session snapshot', () => {
    const commandCall = call({ id: 'command-restart', tool: 'run_command', args: {} })
    const output = {
      name: 'pages/page-1.png',
      kind: 'image' as const,
      readPath: `image-artifact://sha256/${HASH}`,
      mimeType: 'image/png',
      sizeBytes: 2048,
      sha256: HASH,
      width: 1200,
      height: 1600
    }
    const restoredRun = run({
      toolCalls: [commandCall],
      toolResults: [],
      commandSessions: {
        [commandCall.id]: {
          callId: commandCall.id,
          status: 'exited',
          latestSequence: 0,
          outputTruncated: false,
          outputs: [output]
        }
      }
    })

    expect(getImageGenerationArtifactEntries(restoredRun)).toMatchObject([
      {
        callId: commandCall.id,
        displayName: output.name,
        artifact: { uri: output.readPath, sha256: HASH }
      }
    ])
  })
})
