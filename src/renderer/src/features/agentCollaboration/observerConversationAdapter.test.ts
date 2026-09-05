import { describe, expect, it } from 'vitest'
import {
  AGENT_COLLABORATION_SCHEMA_VERSION,
  parseAgentObserverConversation,
  type HumanInteractionResponseDisplay,
  type AgentObserverConversation
} from '@mycopilot/protocol'
import { mapObserverConversationToChat } from './observerConversationAdapter'

function fixture(): AgentObserverConversation {
  return {
    schemaVersion: AGENT_COLLABORATION_SCHEMA_VERSION,
    agentId: 'child-agent',
    rootConversationId: 'root-conversation',
    conversationId: 'child-conversation',
    projectId: 'project-1',
    modelId: 'model-child',
    title: 'Child research',
    createdAt: 10,
    updatedAt: 20,
    messages: [
      {
        messageId: 'parent-task',
        role: 'user',
        content: 'Inspect the implementation.',
        createdAt: 11,
        status: 'sent',
        inputOrigin: {
          kind: 'agent',
          senderAgentId: 'parent-agent',
          sourceAgentMessageId: 'mailbox-message',
          snapshotSourceConversationId: null,
          snapshotSourceMessageId: null
        },
        attachments: [
          {
            attachmentId: 'attachment-1',
            kind: 'image',
            name: 'reference.png',
            mimeType: 'image/png',
            sizeBytes: 12,
            previewData: 'AAAA',
            previewMimeType: 'image/png',
            createdAt: 12
          }
        ],
        agentRunJson: null,
        uiStateJson: '{"favorited":true}'
      },
      {
        messageId: 'child-answer',
        role: 'assistant',
        content: 'Done.',
        createdAt: 13,
        status: 'sent',
        inputOrigin: null,
        attachments: [],
        agentRunJson: null,
        uiStateJson: null
      }
    ]
  }
}

describe('observer conversation adapter', () => {
  it('preserves only verified inherited answer proof and rejects mismatched observer DTO content', () => {
    const response: HumanInteractionResponseDisplay = {
      type: 'human_interaction_response',
      schemaVersion: 1,
      requestId: 'request',
      responseId: 'response',
      answers: [{ kind: 'text', questionId: 'q', question: 'Which?', answer: 'Blue' }]
    }
    const observer = fixture()
    observer.messages[0] = {
      ...observer.messages[0],
      content: JSON.stringify(response),
      humanInteractionResponse: response
    }
    const parsed = parseAgentObserverConversation(observer)!
    expect(mapObserverConversationToChat(parsed).messages[0].humanInteractionDisplay).toEqual(
      response
    )
    observer.messages[0] = { ...observer.messages[0], humanInteractionResponse: undefined }
    expect(
      mapObserverConversationToChat(parseAgentObserverConversation(observer)!).messages[0]
        .humanInteractionDisplay
    ).toBeUndefined()
    observer.messages[0] = {
      ...observer.messages[0],
      humanInteractionResponse: { ...response, requestId: 'foreign' }
    }
    expect(() => parseAgentObserverConversation(observer)).toThrow('complete User content')
  })
  it('maps the exact observer DTO into the existing chat projection without creating draft state', () => {
    const conversation = mapObserverConversationToChat(fixture())

    expect(conversation).toMatchObject({
      id: 'child-conversation',
      projectId: 'project-1',
      modelId: 'model-child',
      messagesLoaded: true,
      pinnedAt: null,
      archivedAt: null,
      unreadAt: null
    })
    expect(conversation.messages[0]).toMatchObject({
      id: 'parent-task',
      role: 'user',
      inputOrigin: {
        kind: 'agent',
        senderAgentId: 'parent-agent',
        sourceAgentMessageId: 'mailbox-message'
      },
      attachments: [
        {
          id: 'attachment-1',
          kind: 'image',
          previewData: 'AAAA',
          encoding: 'base64',
          data: 'AAAA',
          mimeType: 'image/png'
        }
      ]
    })
  })

  it('drops malformed observer UI preferences instead of creating a child write path', () => {
    const observer = fixture()
    observer.messages[0] = {
      ...observer.messages[0],
      uiStateJson: '{"timelineCollapsed":"yes","forged":true}'
    }

    expect(mapObserverConversationToChat(observer).messages[0]?.uiState).toBeUndefined()
  })
})
