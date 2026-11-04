import type {
  AgentConversationTurnInput,
  AgentConversationTurnOutput,
  AgentConversationTurnRewriteInput,
  SkillSelection
} from '@mycopilot/protocol'
import { testState } from './state'

export const skillSelection: SkillSelection = {
  id: 'workspace:project-a:repository-auditor',
  revision: 'skill-sha256-v1:auditor'
}

export const latestSkillSelection: SkillSelection = {
  ...skillSelection,
  revision: 'skill-sha256-v1:auditor-latest'
}

export const explicitSkillSummary = {
  ...skillSelection,
  name: 'Repository auditor',
  source: { id: 'project-a', kind: 'workspace' as const }
}

export const modelSkillSummary = {
  id: 'bundled:application:documents',
  name: 'documents',
  revision: 'skill-package-sha256-v2:documents',
  source: { id: 'bundled:application', kind: 'bundled' as const }
}

export function successfulTurnOutput(
  input: AgentConversationTurnInput,
  runSequence: number
): AgentConversationTurnOutput {
  const conversationId = input.conversationId ?? 'conversation-a'
  const userMessageId = input.userMessageId ?? `user-${runSequence}`
  const assistantMessageId = input.assistantMessageId ?? `assistant-${runSequence}`
  return {
    runId: `run-${runSequence}`,
    eventName: 'agent.event',
    conversationId,
    userMessageId,
    assistantMessageId,
    userMessage: {
      id: userMessageId,
      role: 'user',
      content: input.content,
      createdAt: 10 + runSequence,
      status: 'sent'
    },
    assistantMessage: {
      id: assistantMessageId,
      role: 'assistant',
      content: '',
      createdAt: 20 + runSequence,
      status: 'pending'
    },
    activatedSkills: input.skills?.length ? [explicitSkillSummary] : [],
    skillActivationRevision: input.skills?.length ? 'activation-sha256-v1:explicit' : undefined
  }
}

export function commitSuccessfulRewrite(
  input: AgentConversationTurnRewriteInput,
  runSequence = 100
): AgentConversationTurnOutput {
  const output = successfulTurnOutput(input.turn, runSequence)
  const conversationId = input.turn.conversationId!
  const current = testState.persistedConversations.get(conversationId)
  if (!current) throw new Error('missing rewrite conversation fixture')
  const sourceUserIndex = current.messages.findIndex(
    (message) => message.id === input.sourceUserMessageId
  )
  const sourceAssistantIndex = current.messages.findIndex(
    (message) => message.id === input.sourceAssistantMessageId
  )
  if (sourceUserIndex < 0 || sourceAssistantIndex !== sourceUserIndex + 1) {
    throw new Error('invalid rewrite source fixture')
  }
  testState.persistedConversations.set(conversationId, {
    ...current,
    modelId: input.turn.modelId,
    title: input.turn.title ?? current.title,
    messages: [
      ...current.messages.slice(0, sourceUserIndex),
      {
        id: output.userMessage.id,
        role: 'user',
        content: output.userMessage.content,
        createdAt: output.userMessage.createdAt,
        status: 'sent'
      },
      {
        id: output.assistantMessage.id,
        role: 'assistant',
        content: output.assistantMessage.content,
        createdAt: output.assistantMessage.createdAt,
        status: 'pending',
        agentRun: {
          runId: output.runId,
          status: 'running',
          toolDefinitions: [],
          toolCalls: [],
          toolResults: [],
          approvals: [],
          fileChangeProposals: [],
          timeline: []
        }
      },
      ...current.messages.slice(sourceAssistantIndex + 1)
    ],
    updatedAt: Math.max(
      current.updatedAt + 1,
      output.userMessage.createdAt,
      output.assistantMessage.createdAt
    )
  })
  return output
}

export function mockSuccessfulTurnStarts() {
  let runSequence = 0
  testState.startConversationTurn.mockImplementation(
    async (input: AgentConversationTurnInput): Promise<AgentConversationTurnOutput> => {
      runSequence += 1
      return successfulTurnOutput(input, runSequence)
    }
  )
}
