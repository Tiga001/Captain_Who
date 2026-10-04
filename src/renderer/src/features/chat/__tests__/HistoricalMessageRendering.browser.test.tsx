import { createElement, type ComponentProps } from 'react'
import { beforeEach, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { ChatMessageList } from '../ConversationSurface'
import { ChatMessageItem } from '../components/ChatMessageItem'
import type { AgentRunView } from '../components/AgentRunView'
import type { ChatConversation, ChatMessage } from '../chatTypes'
import type { CollaborationTimelineActivity } from '../../agentCollaboration/collaborationTimelineModel'
import type { HumanInteractionRequestSnapshot } from '@mycopilot/protocol'
import { question } from '../../humanInteraction/__tests__/humanInteractionFixtures'

const probes = vi.hoisted(() => ({ renderBody: vi.fn<(messageId: string) => void>() }))

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ language: 'en-US', t: (key: string) => key })
}))
vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))
vi.mock('../../../components/toast/ToastContext', () => ({
  useToast: () => ({ showToast: vi.fn() })
}))
vi.mock('../components/ImagePreview', () => ({
  useImagePreview: () => vi.fn(),
  useImagePreviewNotice: () => vi.fn()
}))
// Observe the real body component, including its timeline projection and children.
vi.mock('../components/AgentRunView', async (original) => {
  const module = await original<typeof import('../components/AgentRunView')>()
  return {
    ...module,
    AgentRunView: (props: ComponentProps<typeof AgentRunView>) => {
      probes.renderBody(props.message.id)
      return createElement(module.AgentRunView, props)
    }
  }
})

function assistant(index: number): ChatMessage {
  return {
    id: `assistant-${index}`,
    role: 'assistant',
    content: `Final answer ${index}`,
    createdAt: index * 2 + 2,
    status: 'sent',
    agentRun: {
      runId: `run-${index}`,
      status: 'completed',
      startedAt: index * 2 + 1,
      completedAt: index * 2 + 2,
      toolDefinitions: [],
      toolCalls: [
        {
          id: `read-${index}`,
          tool: 'read_file',
          args: { path: 'notes.md' },
          approvalStatus: 'not_required',
          reason: null
        }
      ],
      toolResults: [
        { callId: `read-${index}`, tool: 'read_file', ok: true, result: { path: 'notes.md' } }
      ],
      approvals: [],
      fileChangeProposals: [],
      timeline: [
        { id: `read-item-${index}`, type: 'tool_call', callId: `read-${index}` },
        ...Array.from({ length: 20 }, (_, itemIndex) => ({
          id: `narration-${index}-${itemIndex}`,
          type: 'message' as const,
          content: `Historical narration ${index}/${itemIndex}`
        }))
      ]
    }
  }
}

function history(): ChatConversation {
  return {
    id: 'long-chat',
    projectId: null,
    modelId: 'model',
    title: 'Long chat',
    messages: Array.from({ length: 24 }, (_, index) => [
      {
        id: `user-${index}`,
        role: 'user' as const,
        content: `Question ${index}`,
        createdAt: index * 2 + 1,
        status: 'sent' as const
      },
      assistant(index)
    ]).flat(),
    messagesLoaded: true,
    createdAt: 1,
    updatedAt: 48,
    archivedAt: null,
    unreadAt: null
  }
}

function activity(index: number): CollaborationTimelineActivity {
  return {
    activityId: `activity-${index}`,
    agentId: 'child-agent',
    ownerAgentId: 'root-agent',
    ownerConversationId: 'long-chat',
    anchorMessageId: `assistant-${index}`,
    traceBoundarySequence: 1,
    sequence: index + 1,
    occurredAt: index + 1,
    runId: `run-${index}`,
    semantic: 'completed',
    taskNameSnapshot: 'Researcher',
    taskMessageId: `task-${index}`,
    turnId: null
  }
}

beforeEach(() => probes.renderBody.mockClear())

it('updates historical actions without rerendering their bodies when another turn starts and finishes', async () => {
  const conversation = history()
  const onContinue = vi.fn()
  const onOpenAgent = vi.fn()
  const activities = Array.from({ length: 24 }, (_, index) => activity(index))
  for (const message of conversation.messages) {
    if (message.agentRun) {
      message.agentRun.collaborationTimelineActivities = activities.filter(
        (entry) => entry.anchorMessageId === message.id
      )
    }
  }
  const renderHistory = (current: ChatConversation, running: boolean) => (
    <ChatMessageList
      conversation={current}
      collaborationTimelineActivities={activities}
      editableLastUserMessageId={null}
      editSelectedModelAvailable
      editSelectedModelSupportsImage
      lastAssistantMessageId={current.messages.at(-1)?.id}
      forkDisabledReason={running ? 'Generating' : undefined}
      onContinueInNewTask={onContinue}
      onOpenCollaborationAgent={onOpenAgent}
      showTokenUsageDetails={!running}
    />
  )
  const screen = await render(renderHistory(conversation, false))
  expect(probes.renderBody).toHaveBeenCalledTimes(24)
  expect(screen.container.querySelectorAll('[aria-label="chat.continueInNewTask"]')).toHaveLength(
    24
  )

  const pending = assistant(24)
  pending.status = 'pending'
  pending.content = ''
  pending.agentRun = {
    ...pending.agentRun!,
    status: 'running',
    completedAt: undefined,
    timeline: []
  }
  const started: ChatConversation = {
    ...conversation,
    messages: [
      ...conversation.messages,
      { id: 'new-user', role: 'user', content: 'Continue', createdAt: 49, status: 'sent' },
      pending
    ]
  }
  probes.renderBody.mockClear()
  await screen.rerender(renderHistory(started, true))
  expect(probes.renderBody.mock.calls.map(([messageId]) => messageId)).toEqual([pending.id])
  expect(screen.container.querySelectorAll('[aria-label="chat.continueInNewTask"]')).toHaveLength(0)
  expect(screen.container.textContent).toContain('Final answer 0')

  const completed: ChatConversation = {
    ...started,
    messages: [...started.messages.slice(0, -1), assistant(24)]
  }
  probes.renderBody.mockClear()
  await screen.rerender(renderHistory(completed, false))
  expect(probes.renderBody.mock.calls.map(([messageId]) => messageId)).toEqual([pending.id])
  const historicalFork = screen.container.querySelector<HTMLButtonElement>(
    '[data-message-id="assistant-0"] [aria-label="chat.continueInNewTask"]'
  )
  expect(historicalFork).not.toBeNull()
  historicalFork!.click()
  expect(onContinue).toHaveBeenCalledWith({
    kind: 'assistant_reply',
    assistantMessageId: 'assistant-0'
  })
})

it('updates an older open question when a newer run blocks interaction without rerendering unrelated history', async () => {
  const conversation = history()
  const owner = conversation.messages[1]
  const request: HumanInteractionRequestSnapshot = {
    ...question('historical-question', 1, conversation.id),
    assistantMessageId: owner.id,
    runId: owner.agentRun!.runId!
  }
  owner.agentRun!.toolCalls.push({
    id: request.toolCallId,
    tool: 'request_user_input_async',
    args: { questions: request.questions },
    approvalStatus: 'not_required',
    reason: null
  })
  owner.agentRun!.timeline.push({
    id: 'question-item',
    type: 'tool_call',
    callId: request.toolCallId
  })
  const open = vi.fn()
  const renderHistory = (
    current: ChatConversation,
    requests: HumanInteractionRequestSnapshot[],
    canInteract = true
  ) => (
    <ChatMessageList
      conversation={current}
      editableLastUserMessageId={null}
      editSelectedModelAvailable
      editSelectedModelSupportsImage
      humanInteraction={{ requests, openRequests: requests, canInteract, open }}
      showTokenUsageDetails={false}
    />
  )
  const screen = await render(renderHistory(conversation, [request]))
  const questionButton = () =>
    screen.container.querySelector<HTMLButtonElement>(
      `[data-human-request-id="${request.requestId}"]`
    )!
  expect(questionButton().disabled).toBe(false)
  questionButton().click()
  expect(open).toHaveBeenCalledWith(request.requestId)

  const next = assistant(24)
  next.status = 'pending'
  next.agentRun = { ...next.agentRun!, status: 'running', completedAt: undefined }
  const started = { ...conversation, messages: [...conversation.messages, next] }
  probes.renderBody.mockClear()
  await screen.rerender(renderHistory(started, [request]))
  expect(probes.renderBody.mock.calls.map(([id]) => id)).toEqual([owner.id, next.id])
  expect(questionButton().disabled).toBe(false)

  const blockingRequest: HumanInteractionRequestSnapshot = {
    ...question('blocking-question', 2, conversation.id),
    assistantMessageId: next.id,
    runId: next.agentRun!.runId!,
    mode: 'sync'
  }
  probes.renderBody.mockClear()
  await screen.rerender(renderHistory(started, [request, blockingRequest]))
  expect(probes.renderBody.mock.calls.map(([id]) => id)).toEqual([owner.id, next.id])
  expect(questionButton().disabled).toBe(true)

  probes.renderBody.mockClear()
  await screen.rerender(renderHistory(started, [request], false))
  expect(probes.renderBody.mock.calls.map(([id]) => id)).toEqual([owner.id, next.id])
  expect(questionButton().disabled).toBe(true)

  probes.renderBody.mockClear()
  await screen.rerender(renderHistory(started, [request]))
  expect(probes.renderBody.mock.calls.map(([id]) => id)).toEqual([owner.id])
  expect(questionButton().disabled).toBe(false)
})

it('still refreshes changed content and timeline expansion while ignoring toolbar-only changes', async () => {
  const message = assistant(0)
  const onContinue = vi.fn()
  const renderMessage = (current: ChatMessage, collapsed: boolean, showActions: boolean) => (
    <ChatMessageItem
      message={current}
      mode="observer"
      onContinueInNewTask={showActions ? onContinue : undefined}
      timelineCollapsedOverride={collapsed}
      showTokenUsageDetails={showActions}
    />
  )
  const screen = await render(renderMessage(message, true, true))
  expect(probes.renderBody).toHaveBeenCalledTimes(1)
  expect(screen.container.textContent).not.toContain('Historical narration 0/0')

  await screen.rerender(renderMessage(message, true, false))
  expect(probes.renderBody).toHaveBeenCalledTimes(1)

  await screen.rerender(renderMessage(message, false, false))
  expect(probes.renderBody).toHaveBeenCalledTimes(2)
  expect(screen.container.textContent).toContain('Historical narration 0/0')

  await screen.rerender(renderMessage({ ...message, content: 'Corrected answer' }, false, false))
  expect(probes.renderBody).toHaveBeenCalledTimes(3)
  expect(screen.container.textContent).toContain('Corrected answer')
  expect(screen.container.textContent).not.toContain('Final answer 0')
})
