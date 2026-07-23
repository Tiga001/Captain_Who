// Browser coverage that image-generation UI never exposes redacted or private Tool fields.
import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ChatAgentRunView } from '../../features/chat/chatTypes'
import { ImageGenerationArtifactsCard } from '../../features/chat/components/ImageGenerationArtifactsCard'
import { ImageGenerationToolActivity } from '../../features/chat/components/toolActivities/ImageGenerationToolActivity'
import type { ResolvedImageArtifact } from '../../features/imageGeneration/artifacts/ImageArtifactResolver'

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))

const HASH = 'b'.repeat(64)

function call(overrides: Partial<AgentToolCall> = {}): AgentToolCall {
  return {
    id: 'image-call',
    tool: 'image_generation',
    args: { request: { operation: 'generate', hasInputImage: false }, reason: 'Create a poster' },
    approvalStatus: 'not_required',
    ...overrides
  }
}

function succeededResult(overrides: Partial<AgentToolResult> = {}): AgentToolResult {
  return {
    callId: 'image-call',
    tool: 'image_generation',
    ok: true,
    result: {
      schemaVersion: 1,
      status: 'succeeded',
      operation: 'generate',
      artifact: {
        artifactId: `sha256:${HASH}`,
        uri: `image-artifact://sha256/${HASH}`,
        kind: 'image',
        format: 'webp',
        mimeType: 'image/webp',
        width: 1536,
        height: 1024,
        sizeBytes: 4096,
        sha256: HASH
      },
      audit: {
        executionId: 'execution-1',
        requestFingerprint: `sha256:${HASH}`,
        providerProfileId: 'profile-1',
        adapterId: 'smartmlSeedream',
        profileRevision: 1,
        modelId: 'private-provider-model',
        createdAt: 100,
        completedAt: 120,
        durationMs: 20
      }
    },
    ...overrides
  }
}

function run(toolCalls: AgentToolCall[], toolResults: AgentToolResult[]): ChatAgentRunView {
  return {
    runId: 'run-1',
    status: 'completed',
    toolDefinitions: [],
    toolCalls,
    toolResults,
    approvals: [],
    diffs: [],
    timeline: []
  }
}

function deferred<Value>() {
  let resolve!: (value: Value) => void
  const promise = new Promise<Value>((resolvePromise) => {
    resolve = resolvePromise
  })
  return { promise, resolve }
}

describe('image generation activity UI', () => {
  it('uses a safe fallback for preflight failures and never renders raw Tool fields', async () => {
    const privateCall = call({
      args: {
        request: {
          operation: 'edit',
          hasInputImage: true,
          prompt: 'private prompt',
          inputPath: '/Users/example/private.png'
        },
        reason: null
      }
    })
    const screen = await render(
      <ImageGenerationToolActivity
        call={privateCall}
        result={{
          callId: privateCall.id,
          tool: 'image_generation',
          ok: false,
          result: { providerUrl: 'https://provider.private', apiKey: 'secret-value' }
        }}
      />
    )

    await expect.element(screen.getByText('agent.imageGeneration.edit.failed')).toBeVisible()
    await screen.getByText('agent.imageGeneration.edit.failed').click()
    await expect.element(screen.getByText('agent.imageGeneration.safeFailure')).toBeVisible()
    expect(screen.container.textContent).not.toContain('private prompt')
    expect(screen.container.textContent).not.toContain('/Users/example/private.png')
    expect(screen.container.textContent).not.toContain('provider.private')
    expect(screen.container.textContent).not.toContain('secret-value')
  })

  it('shows deduplicated metadata without fabricating preview or export controls', async () => {
    const firstCall = call()
    const secondCall = call({ id: 'image-call-2' })
    const firstResult = succeededResult()
    const currentRun = run(
      [firstCall, secondCall],
      [firstResult, succeededResult({ callId: secondCall.id })]
    )
    const screen = await render(<ImageGenerationArtifactsCard run={currentRun} />)

    await expect.element(screen.getByText('agent.imageGeneration.previewUnavailable')).toBeVisible()
    expect(screen.container.querySelectorAll('.image-generation-artifact-card')).toHaveLength(1)
    expect(screen.container.querySelector('img')).toBeNull()
    expect(screen.container.querySelector('a')).toBeNull()
    expect(screen.container.querySelector('button')).toBeNull()
    expect(screen.container.textContent).not.toContain(HASH)
    expect(screen.container.textContent).not.toContain('private-provider-model')
  })

  it('keeps a future trusted Resolver in an explicit loading state', async () => {
    const pending = deferred<ResolvedImageArtifact>()
    const currentRun = run([call()], [succeededResult()])
    const screen = await render(
      <ImageGenerationArtifactsCard
        resolver={{ resolve: vi.fn(() => pending.promise) }}
        run={currentRun}
      />
    )

    await expect.element(screen.getByText('agent.imageGeneration.previewLoading')).toBeVisible()
    expect(screen.container.querySelector('img')).toBeNull()
    pending.resolve({ src: 'blob:trusted-image-preview' })
    await expect.poll(() => screen.container.querySelector('img')).not.toBeNull()
  })
})
