// Browser regressions for stable Timeline identity and Conversation-scoped image Artifact leases.
import type {
  AgentImageGenerationArtifact,
  AgentToolCall,
  AgentToolResult
} from '@mycopilot/protocol'
import { StrictMode } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import type { ChatAgentRunView, ChatMessage } from '../../features/chat/chatTypes'
import { ChatMessageItem } from '../../features/chat/components/ChatMessageItem'
import { ImageGenerationArtifactsCard } from '../../features/chat/components/ImageGenerationArtifactsCard'

const mocks = vi.hoisted(() => ({
  openImagePreview: vi.fn(),
  readArtifact: vi.fn()
}))

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'zh-CN',
    t: (key: string) => key
  })
}))

vi.mock('../../features/storage/storageClient', () => ({
  loadAttachmentImage: vi.fn(),
  loadImageFile: vi.fn(),
  revealStoredProjectFile: vi.fn()
}))

vi.mock('../../features/chat/components/ImagePreview', () => ({
  useImagePreview: () => mocks.openImagePreview,
  useImagePreviewNotice: () => vi.fn()
}))

vi.mock('../../host/hostClient', () => ({
  hostClient: {
    imageGeneration: {
      readArtifact: mocks.readArtifact
    }
  }
}))

const FIRST_HASH = 'a'.repeat(64)
const SECOND_HASH = 'b'.repeat(64)
const PREVIEW_DATA_URL =
  'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII='

function imageArtifact(hash = FIRST_HASH): AgentImageGenerationArtifact {
  return {
    artifactId: `sha256:${hash}`,
    uri: `image-artifact://sha256/${hash}`,
    kind: 'image',
    format: 'png',
    mimeType: 'image/png',
    width: 1200,
    height: 800,
    sizeBytes: 4,
    sha256: hash
  }
}

function imageCall(): AgentToolCall {
  return {
    id: 'image-call',
    tool: 'image_generation',
    args: {
      request: { operation: 'generate', hasInputImage: false },
      reason: '生成演示文稿配图。'
    },
    approvalStatus: 'not_required',
    reason: '生成演示文稿配图。'
  }
}

function imageResult(hash = FIRST_HASH): AgentToolResult {
  const artifact = imageArtifact(hash)
  return {
    callId: 'image-call',
    tool: 'image_generation',
    ok: true,
    result: {
      schemaVersion: 1,
      status: 'succeeded',
      operation: 'generate',
      artifact,
      audit: {
        executionId: `agent-v1:${'c'.repeat(64)}`,
        requestFingerprint: `sha256:${'d'.repeat(64)}`,
        providerProfileId: 'profile-1',
        adapterId: 'smartmlSeedream',
        profileRevision: 1,
        modelId: 'image-model',
        createdAt: 10,
        completedAt: 20,
        durationMs: 10
      }
    }
  }
}

function officeCheck(id: string): AgentToolCall {
  return {
    id,
    tool: 'office_presentation',
    args: { request: { operation: 'validate', path: '四川大学介绍.pptx' } },
    approvalStatus: 'not_required',
    reason: '检查演示文稿。'
  }
}

function runningMessage(officeCallIds: readonly string[]): ChatMessage {
  const toolCalls = [imageCall(), ...officeCallIds.map(officeCheck)]
  return {
    id: 'assistant-running',
    role: 'assistant',
    content: '',
    createdAt: 1,
    status: 'pending',
    agentRun: {
      runId: 'run-running',
      status: 'running',
      startedAt: 1,
      firstResponseAt: 2,
      toolDefinitions: [],
      toolCalls,
      toolResults: [
        imageResult(),
        {
          callId: officeCallIds[0] ?? 'office-check-1',
          tool: 'office_presentation',
          ok: true,
          result: { status: 'completed' }
        }
      ],
      approvals: [],
      diffs: [],
      timeline: toolCalls.map((call, index) => ({
        id: `timeline-${call.id}`,
        type: 'tool_call' as const,
        callId: call.id,
        traceSequence: index
      }))
    }
  }
}

function settledRun(hash = FIRST_HASH): ChatAgentRunView {
  return {
    runId: 'run-settled',
    status: 'completed',
    startedAt: 1,
    completedAt: 2,
    toolDefinitions: [],
    toolCalls: [imageCall()],
    toolResults: [imageResult(hash)],
    approvals: [],
    diffs: [],
    timeline: [{ id: 'timeline-image-call', type: 'tool_call', callId: 'image-call' }]
  }
}

function settledMessage(hash = FIRST_HASH): ChatMessage {
  return {
    id: 'assistant-settled',
    role: 'assistant',
    content: '演示文稿已经完成。',
    createdAt: 1,
    status: 'sent',
    agentRun: settledRun(hash)
  }
}

describe('Timeline and image Artifact stability', () => {
  const originalIntersectionObserver = window.IntersectionObserver

  beforeEach(() => {
    vi.clearAllMocks()
    // The preload boundary is orthogonal to these identity regressions. Resolve immediately while
    // still exercising the production Host resolver and object-URL lifecycle.
    window.IntersectionObserver = undefined as unknown as typeof IntersectionObserver
    mocks.readArtifact.mockImplementation(
      async (request: { artifact: AgentImageGenerationArtifact }) => ({
        ok: true,
        value: {
          schemaVersion: 1,
          artifact: request.artifact,
          fileName: 'generated.png',
          bytes: Uint8Array.from([1, 2, 3, 4])
        }
      })
    )
  })

  afterEach(() => {
    window.IntersectionObserver = originalIntersectionObserver
    vi.restoreAllMocks()
  })

  it('keeps an existing image Timeline item mounted when an Office check is appended', async () => {
    let objectUrlSequence = 0
    const createObjectUrl = vi
      .spyOn(URL, 'createObjectURL')
      .mockImplementation(() => `${PREVIEW_DATA_URL}#timeline-${++objectUrlSequence}`)
    const revokeObjectUrl = vi.spyOn(URL, 'revokeObjectURL').mockImplementation(() => undefined)
    const props = {
      conversationId: 'child-conversation',
      mode: 'observer' as const,
      observerRootConversationId: 'root-conversation',
      projectId: 'project-1',
      showTokenUsageDetails: false
    }
    const screen = await render(
      <ChatMessageItem {...props} message={runningMessage(['office-check-1'])} />
    )

    await expect
      .poll(() =>
        screen.container.querySelector<HTMLImageElement>('.image-generation-activity__preview img')
      )
      .not.toBeNull()
    const imageBefore = screen.container.querySelector<HTMLImageElement>(
      '.image-generation-activity__preview img'
    )
    const disclosureBefore = screen.container.querySelector<HTMLDetailsElement>(
      '.agent-activity--image-generation'
    )
    expect(imageBefore?.src).toContain('#timeline-1')
    expect(disclosureBefore?.open).toBe(true)
    await userEvent.click(disclosureBefore!.querySelector('summary')!)
    await expect.poll(() => disclosureBefore?.open).toBe(false)

    // This changes the old Timeline segment range from [0, 2) to [0, 3), while the image item and
    // its immutable Artifact are unchanged and every persisted run object is newly restored.
    await screen.rerender(
      <ChatMessageItem
        {...props}
        message={structuredClone(runningMessage(['office-check-1', 'office-check-2']))}
      />
    )

    const imageAfter = screen.container.querySelector<HTMLImageElement>(
      '.image-generation-activity__preview img'
    )
    const disclosureAfter = screen.container.querySelector<HTMLDetailsElement>(
      '.agent-activity--image-generation'
    )
    expect(imageAfter).toBe(imageBefore)
    expect(imageAfter?.src).toBe(imageBefore?.src)
    expect(disclosureAfter).toBe(disclosureBefore)
    expect(disclosureAfter?.open).toBe(false)
    expect(mocks.readArtifact).toHaveBeenCalledTimes(1)
    expect(mocks.readArtifact).toHaveBeenCalledWith(
      expect.objectContaining({
        conversationId: 'child-conversation',
        observerRootConversationId: 'root-conversation'
      })
    )
    expect(createObjectUrl).toHaveBeenCalledTimes(1)
    expect(revokeObjectUrl).not.toHaveBeenCalled()

    screen.unmount()
    await expect.poll(() => revokeObjectUrl.mock.calls.length).toBe(1)
  })

  it('reuses a settled gallery across restored run objects but not across authority or Artifact changes', async () => {
    let objectUrlSequence = 0
    const createObjectUrl = vi
      .spyOn(URL, 'createObjectURL')
      .mockImplementation(() => `${PREVIEW_DATA_URL}#gallery-${++objectUrlSequence}`)
    const revokeObjectUrl = vi.spyOn(URL, 'revokeObjectURL').mockImplementation(() => undefined)
    const screen = await render(
      <ChatMessageItem
        conversationId="child-a"
        message={settledMessage()}
        mode="observer"
        observerRootConversationId="root-a"
        showTokenUsageDetails={false}
      />
    )
    await expect
      .poll(() =>
        screen.container.querySelector<HTMLImageElement>('.image-generation-artifact-preview img')
      )
      .not.toBeNull()
    const firstImage = screen.container.querySelector<HTMLImageElement>(
      '.image-generation-artifact-preview img'
    )
    expect(firstImage?.src).toContain('#gallery-1')

    await screen.rerender(
      <ChatMessageItem
        conversationId="child-a"
        message={structuredClone(settledMessage())}
        mode="observer"
        observerRootConversationId="root-a"
        showTokenUsageDetails={false}
      />
    )
    await expect
      .poll(() =>
        screen.container.querySelector<HTMLImageElement>('.image-generation-artifact-preview img')
      )
      .not.toBeNull()
    expect(
      screen.container.querySelector<HTMLImageElement>('.image-generation-artifact-preview img')
    ).toBe(firstImage)
    expect(mocks.readArtifact).toHaveBeenCalledTimes(1)
    expect(createObjectUrl).toHaveBeenCalledTimes(1)
    expect(revokeObjectUrl).not.toHaveBeenCalled()

    await screen.rerender(
      <ChatMessageItem
        conversationId="child-b"
        message={structuredClone(settledMessage())}
        mode="observer"
        observerRootConversationId="root-a"
        showTokenUsageDetails={false}
      />
    )
    await expect.poll(() => mocks.readArtifact.mock.calls.length).toBe(2)
    await expect.poll(() => revokeObjectUrl.mock.calls.length).toBe(1)
    expect(mocks.readArtifact.mock.calls[1]?.[0]).toEqual(
      expect.objectContaining({ conversationId: 'child-b', observerRootConversationId: 'root-a' })
    )

    await screen.rerender(
      <ChatMessageItem
        conversationId="child-b"
        message={structuredClone(settledMessage())}
        mode="observer"
        observerRootConversationId="root-b"
        showTokenUsageDetails={false}
      />
    )
    await expect.poll(() => mocks.readArtifact.mock.calls.length).toBe(3)
    await expect.poll(() => revokeObjectUrl.mock.calls.length).toBe(2)
    expect(mocks.readArtifact.mock.calls[2]?.[0]).toEqual(
      expect.objectContaining({ conversationId: 'child-b', observerRootConversationId: 'root-b' })
    )

    await screen.rerender(
      <ChatMessageItem
        conversationId="child-b"
        message={structuredClone(settledMessage(SECOND_HASH))}
        mode="observer"
        observerRootConversationId="root-b"
        showTokenUsageDetails={false}
      />
    )
    await expect.poll(() => mocks.readArtifact.mock.calls.length).toBe(4)
    await expect.poll(() => revokeObjectUrl.mock.calls.length).toBe(3)
    expect(mocks.readArtifact.mock.calls[3]?.[0]).toEqual(
      expect.objectContaining({
        artifact: expect.objectContaining({ artifactId: `sha256:${SECOND_HASH}` })
      })
    )

    screen.unmount()
    await expect.poll(() => revokeObjectUrl.mock.calls.length).toBe(4)
  })

  it('treats resolver identity as part of the component-local Artifact lease', async () => {
    const releaseFirst = vi.fn()
    const releaseSecond = vi.fn()
    const firstResolver = {
      resolve: vi.fn(async () => ({ release: releaseFirst, src: 'blob:first-resolver' }))
    }
    const secondResolver = {
      resolve: vi.fn(async () => ({ release: releaseSecond, src: 'blob:second-resolver' }))
    }
    const screen = await render(
      <ImageGenerationArtifactsCard
        conversationId="conversation-1"
        resolver={firstResolver}
        run={settledRun()}
      />
    )

    await expect.poll(() => firstResolver.resolve.mock.calls.length).toBe(1)
    await screen.rerender(
      <ImageGenerationArtifactsCard
        conversationId="conversation-1"
        resolver={firstResolver}
        run={structuredClone(settledRun())}
      />
    )
    expect(firstResolver.resolve).toHaveBeenCalledTimes(1)
    expect(releaseFirst).not.toHaveBeenCalled()

    await screen.rerender(
      <ImageGenerationArtifactsCard
        conversationId="conversation-1"
        resolver={secondResolver}
        run={structuredClone(settledRun())}
      />
    )
    await expect.poll(() => secondResolver.resolve.mock.calls.length).toBe(1)
    expect(releaseFirst).toHaveBeenCalledTimes(1)

    screen.unmount()
    await expect.poll(() => releaseSecond.mock.calls.length).toBe(1)
  })

  it('recreates a disposed request during the StrictMode effect probe and releases every lease', async () => {
    const releases: ReturnType<typeof vi.fn>[] = []
    const resolver = {
      resolve: vi.fn(async () => {
        const release = vi.fn()
        releases.push(release)
        return { release, src: `blob:strict-${releases.length}` }
      })
    }
    const screen = await render(
      <StrictMode>
        <ImageGenerationArtifactsCard resolver={resolver} run={settledRun()} />
      </StrictMode>
    )

    await expect
      .poll(() =>
        screen.container.querySelector<HTMLImageElement>('.image-generation-artifact-preview img')
      )
      .not.toBeNull()
    expect(resolver.resolve.mock.calls.length).toBeGreaterThanOrEqual(1)
    expect(releases.filter((release) => release.mock.calls.length === 0)).toHaveLength(1)

    screen.unmount()
    await expect
      .poll(() => releases.filter((release) => release.mock.calls.length === 0).length)
      .toBe(0)
  })
})
