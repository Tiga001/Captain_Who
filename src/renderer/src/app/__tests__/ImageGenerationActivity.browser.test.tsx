// Browser coverage that image-generation UI never exposes redacted or private Tool fields.
import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ChatAgentRunView } from '../../features/chat/chatTypes'
import { ImageGenerationArtifactsCard } from '../../features/chat/components/ImageGenerationArtifactsCard'
import { ImageGenerationToolActivity } from '../../features/chat/components/toolActivities/ImageGenerationToolActivity'
import type { ResolvedImageArtifact } from '../../features/imageGeneration/artifacts/ImageArtifactResolver'

const mocks = vi.hoisted(() => ({
  openImagePreview: vi.fn()
}))

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))

vi.mock('../../features/chat/components/ImagePreview', () => ({
  useImagePreview: () => mocks.openImagePreview
}))

const HASH = 'b'.repeat(64)
const PREVIEW_DATA_URL =
  'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII='

function call(overrides: Partial<AgentToolCall> = {}): AgentToolCall {
  return {
    id: 'image-call',
    tool: 'image_generation',
    args: { request: { operation: 'generate', hasInputImage: false }, reason: 'Create a poster' },
    approvalStatus: 'not_required',
    ...overrides
  }
}

function succeededResult(overrides: Partial<AgentToolResult> = {}, hash = HASH): AgentToolResult {
  return {
    callId: 'image-call',
    tool: 'image_generation',
    ok: true,
    result: {
      schemaVersion: 1,
      status: 'succeeded',
      operation: 'generate',
      artifact: {
        artifactId: `sha256:${hash}`,
        uri: `image-artifact://sha256/${hash}`,
        kind: 'image',
        format: 'webp',
        mimeType: 'image/webp',
        width: 1536,
        height: 1024,
        sizeBytes: 4096,
        sha256: hash
      },
      audit: {
        executionId: 'execution-1',
        requestFingerprint: `sha256:${hash}`,
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
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('shows a stable animated thumbnail placeholder while generation is running', async () => {
    const resolver = { resolve: vi.fn() }
    const screen = await render(<ImageGenerationToolActivity call={call()} resolver={resolver} />)

    await expect.element(screen.getByText('agent.imageGeneration.generate.running')).toBeVisible()
    expect(screen.container.querySelector('.agent-activity__icon svg')).toHaveClass('lucide-images')
    expect(
      screen.container
        .querySelector('.image-generation-activity__preview')
        ?.getAttribute('data-status')
    ).toBe('loading')
    expect(screen.container.querySelectorAll('.image-generation-activity__wave i')).toHaveLength(3)
    expect(screen.container.querySelector('details')?.open).toBe(true)
    expect(screen.container.querySelector('img')).toBeNull()
    expect(resolver.resolve).not.toHaveBeenCalled()
  })

  it('replaces the placeholder with a thumbnail and opens the shared viewer', async () => {
    const imageCall = call()
    const resolver = {
      resolve: vi.fn(async () => ({ src: PREVIEW_DATA_URL }))
    }
    const screen = await render(
      <ImageGenerationToolActivity call={imageCall} resolver={resolver} />
    )

    expect(
      screen.container
        .querySelector('.image-generation-activity__preview')
        ?.getAttribute('data-status')
    ).toBe('loading')
    await screen.rerender(
      <ImageGenerationToolActivity
        call={imageCall}
        resolver={resolver}
        result={succeededResult()}
      />
    )

    expect(screen.container.querySelector('.agent-activity__icon svg')).toHaveClass('lucide-images')
    await expect.poll(() => screen.container.querySelector('img')).not.toBeNull()
    expect(
      screen.container
        .querySelector('.image-generation-activity__preview')
        ?.getAttribute('data-status')
    ).toBe('ready')
    screen.container
      .querySelector<HTMLButtonElement>('.image-generation-activity__preview > button')
      ?.click()
    expect(mocks.openImagePreview).toHaveBeenCalledWith({
      alt: 'agent.imageGeneration.artifact',
      fileName: `generated-image-${HASH.slice(0, 12)}.webp`,
      src: PREVIEW_DATA_URL
    })
    expect(resolver.resolve).toHaveBeenCalledTimes(1)
  })

  it('keeps only the image description in a settled Tool activity', async () => {
    const imageCall = call()
    const resolver = {
      resolve: vi.fn(async () => ({ src: PREVIEW_DATA_URL }))
    }
    const screen = await render(
      <ImageGenerationToolActivity
        call={imageCall}
        resolver={resolver}
        result={succeededResult()}
        showArtifactPreview={false}
      />
    )

    await expect.element(screen.getByText('Create a poster')).toBeVisible()
    expect(screen.container.querySelector('.image-generation-activity__preview')).toBeNull()
    expect(resolver.resolve).not.toHaveBeenCalled()
  })

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
    expect(screen.container.querySelector('.image-generation-activity__preview')).toBeNull()
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
    pending.resolve({ src: PREVIEW_DATA_URL })
    await expect.poll(() => screen.container.querySelector('img')).not.toBeNull()
  })

  it('renders a resolved image directly and opens it in the shared image viewer', async () => {
    const currentRun = run([call()], [succeededResult()])
    const resolver = {
      resolve: vi.fn(async () => ({ src: PREVIEW_DATA_URL }))
    }
    const screen = await render(
      <ImageGenerationArtifactsCard resolver={resolver} run={currentRun} />
    )

    await expect.poll(() => screen.container.querySelectorAll('button').length).toBe(2)
    screen.container
      .querySelector<HTMLButtonElement>('.image-generation-artifact-preview > button')
      ?.click()
    await vi.waitFor(() => {
      expect(mocks.openImagePreview).toHaveBeenCalledWith({
        alt: 'agent.imageGeneration.artifact',
        fileName: `generated-image-${HASH.slice(0, 12)}.webp`,
        src: PREVIEW_DATA_URL
      })
    })
    mocks.openImagePreview.mockClear()
    screen.container
      .querySelector<HTMLButtonElement>('.image-generation-artifact-card > button')
      ?.click()
    expect(mocks.openImagePreview).toHaveBeenCalledWith({
      alt: 'agent.imageGeneration.artifact',
      fileName: `generated-image-${HASH.slice(0, 12)}.webp`,
      src: PREVIEW_DATA_URL
    })
    expect(screen.container.querySelectorAll('.image-generation-artifact-card')).toHaveLength(1)
    expect(screen.container.querySelector('a')).toBeNull()
    expect(resolver.resolve).toHaveBeenCalledTimes(1)
  })

  it('reuses the image card for managed command outputs with conversation authority', async () => {
    const commandCall = call({ id: 'command-image', tool: 'run_command', args: {} })
    const currentRun = {
      ...run([commandCall], []),
      commandSessions: {
        [commandCall.id]: {
          callId: commandCall.id,
          status: 'exited' as const,
          latestSequence: 0,
          outputTruncated: false,
          exitCode: 0,
          outputs: [
            {
              name: 'pages/page-307.png',
              kind: 'image' as const,
              readPath: `image-artifact://sha256/${HASH}`,
              mimeType: 'image/png',
              sizeBytes: 4096,
              sha256: HASH,
              width: 1200,
              height: 1600
            }
          ]
        }
      }
    }
    const resolver = { resolve: vi.fn(async () => ({ src: PREVIEW_DATA_URL })) }
    const screen = await render(
      <ImageGenerationArtifactsCard
        conversationId="conversation-1"
        resolver={resolver}
        run={currentRun}
      />
    )

    await expect.poll(() => screen.container.querySelector('img')).not.toBeNull()
    await expect.element(screen.getByText('pages/page-307.png')).toBeVisible()
    expect(resolver.resolve).toHaveBeenCalledWith(
      expect.objectContaining({ uri: `image-artifact://sha256/${HASH}` }),
      expect.objectContaining({ conversationId: 'conversation-1' })
    )
  })

  it('joins multiple resolved Artifacts into one image grid', async () => {
    const secondHash = 'c'.repeat(64)
    const thirdHash = 'd'.repeat(64)
    const calls = [
      call(),
      call({ id: 'image-call-2' }),
      call({ id: 'image-call-3', args: { request: { operation: 'edit', hasInputImage: true } } })
    ]
    const currentRun = run(calls, [
      succeededResult(),
      succeededResult({ callId: 'image-call-2' }, secondHash),
      succeededResult({ callId: 'image-call-3' }, thirdHash)
    ])
    const resolver = {
      resolve: vi.fn(async () => ({ src: PREVIEW_DATA_URL }))
    }
    const screen = await render(
      <ImageGenerationArtifactsCard resolver={resolver} run={currentRun} />
    )

    await expect.poll(() => screen.container.querySelectorAll('img').length).toBe(6)
    const grid = screen.container.querySelector('.image-generation-artifact-section')
    expect(grid?.getAttribute('data-count')).toBe('3')
    expect(screen.container.querySelectorAll('.image-generation-artifact-gallery')).toHaveLength(1)
    expect(screen.container.querySelectorAll('.image-generation-artifact-list')).toHaveLength(1)
    expect(screen.container.querySelectorAll('.image-generation-artifact-card')).toHaveLength(3)
    expect(screen.container.querySelectorAll('button')).toHaveLength(6)
    expect(resolver.resolve).toHaveBeenCalledTimes(3)
  })

  it('falls back to a non-interactive placeholder when Artifact resolution fails', async () => {
    const currentRun = run([call()], [succeededResult()])
    const screen = await render(
      <ImageGenerationArtifactsCard
        resolver={{ resolve: vi.fn(async () => Promise.reject(new Error('unavailable'))) }}
        run={currentRun}
      />
    )

    await expect.element(screen.getByText('agent.imageGeneration.previewFailed')).toBeVisible()
    expect(screen.container.querySelector('img')).toBeNull()
    expect(screen.container.querySelector('button')).toBeNull()
    expect(mocks.openImagePreview).not.toHaveBeenCalled()
  })

  it('loads Artifacts once near the viewport and keeps them until the message unmounts', async () => {
    const originalIntersectionObserver = window.IntersectionObserver
    let observerCallback: IntersectionObserverCallback | undefined
    let observerInstance: IntersectionObserver | undefined
    const observe = vi.fn()
    const captureObserver = (observer: IntersectionObserver) => {
      observerInstance = observer
    }

    class ControlledIntersectionObserver implements IntersectionObserver {
      readonly root = null
      readonly rootMargin = '800px 0px'
      readonly thresholds = [0]
      disconnect = vi.fn()
      unobserve = vi.fn()

      constructor(callback: IntersectionObserverCallback) {
        observerCallback = callback
        captureObserver(this)
      }

      observe = observe
      takeRecords(): IntersectionObserverEntry[] {
        return []
      }
    }

    window.IntersectionObserver = ControlledIntersectionObserver
    const release = vi.fn()
    const resolver = {
      resolve: vi.fn(async () => ({ release, src: PREVIEW_DATA_URL }))
    }
    try {
      const screen = await render(
        <ImageGenerationArtifactsCard
          resolver={resolver}
          run={run([call()], [succeededResult()])}
        />
      )
      await vi.waitFor(() => expect(observe).toHaveBeenCalledOnce())
      expect(resolver.resolve).not.toHaveBeenCalled()

      observerCallback?.(
        [{ isIntersecting: true } as IntersectionObserverEntry],
        observerInstance as IntersectionObserver
      )
      await vi.waitFor(() => expect(resolver.resolve).toHaveBeenCalledOnce())

      observerCallback?.(
        [{ isIntersecting: false } as IntersectionObserverEntry],
        observerInstance as IntersectionObserver
      )
      await vi.waitFor(() => expect(resolver.resolve).toHaveBeenCalledOnce())
      expect(release).not.toHaveBeenCalled()
      expect(screen.container.querySelector('img')).not.toBeNull()

      screen.unmount()
      await vi.waitFor(() => expect(release).toHaveBeenCalledOnce())
    } finally {
      window.IntersectionObserver = originalIntersectionObserver
    }
  })
})
