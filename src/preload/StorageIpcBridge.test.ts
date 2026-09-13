import type { IpcRenderer } from 'electron'
import type { HostInvocationResult } from '@mycopilot/host-api'
import type { StorageChatConversationRecord } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { createStorageIpcBridge } from './StorageIpcBridge'

describe('Storage IPC bridge', () => {
  it('routes Provider Profile descriptors and authoritative model saves over dedicated channels', async () => {
    const descriptors = [
      {
        profileId: 'deepseek_v4_1_flash_chat',
        profileVersion: 1,
        displayName: 'DeepSeek Flash',
        compatibleDialects: ['openai_chat_completions'],
        settingsKind: 'none',
        selectable: false
      }
    ]
    const authoritativeSettings = {
      configurationRevision: 'model-settings-v1:00000000-0000-4000-8000-000000000001',
      apiUrl: '',
      apiTokenStatus: 'missing' as const,
      searchMode: 'auto',
      tavilyApiKeyStatus: 'missing' as const,
      models: []
    }
    const update = {
      expectedRevision: null,
      apiUrl: '',
      apiTokenMutation: { type: 'keep' as const },
      searchMode: 'auto',
      tavilyApiKeyMutation: { type: 'keep' as const },
      models: []
    }
    const invoke = vi
      .fn()
      .mockResolvedValueOnce(descriptors)
      .mockResolvedValueOnce({ ok: true, value: authoritativeSettings })
    const bridge = createStorageIpcBridge({ invoke } as unknown as Pick<
      IpcRenderer,
      'invoke' | 'on' | 'removeListener'
    >)

    await expect(bridge.loadProviderProfileUiDescriptors()).resolves.toEqual(descriptors)
    await expect(bridge.saveModelSettings(update)).resolves.toEqual({
      ok: true,
      value: authoritativeSettings
    })
    expect(invoke).toHaveBeenNthCalledWith(1, 'host:storage.loadProviderProfileUiDescriptors')
    expect(invoke).toHaveBeenNthCalledWith(2, 'host:storage.saveModelSettings', update)
  })

  it('routes the safe Provider vendor directory and model policy resolver', async () => {
    const descriptors = [
      { vendorId: 'generic', displayName: 'Generic', selectable: true },
      { vendorId: 'deepseek', displayName: 'DeepSeek', selectable: true },
      { vendorId: 'moonshot', displayName: 'Moonshot AI', selectable: true }
    ]
    const policy = {
      status: 'supported',
      vendorId: 'moonshot',
      modelFamily: 'moonshot_k3_chat',
      settingsKind: 'moonshot',
      imageInput: 'supported',
      settings: {
        kind: 'moonshot_k3_chat',
        reasoningEfforts: ['provider_default', 'low', 'high', 'max'],
        defaultSettings: { kind: 'moonshot_k3_chat', reasoningEffort: 'max' }
      }
    }
    const input = {
      vendorId: 'moonshot',
      modelId: 'kimi-k3',
      dialect: 'openai_chat_completions'
    } as const
    const invoke = vi.fn().mockResolvedValueOnce(descriptors).mockResolvedValueOnce(policy)
    const bridge = createStorageIpcBridge({ invoke } as unknown as Pick<
      IpcRenderer,
      'invoke' | 'on' | 'removeListener'
    >)

    await expect(bridge.loadProviderVendorDescriptors()).resolves.toEqual(descriptors)
    await expect(bridge.resolveProviderVendorModelPolicy(input)).resolves.toEqual(policy)
    expect(invoke).toHaveBeenNthCalledWith(1, 'host:storage.loadProviderVendorDescriptors')
    expect(invoke).toHaveBeenNthCalledWith(
      2,
      'host:storage.resolveProviderVendorModelPolicy',
      input
    )
  })

  it('rejects private or unknown fields in Provider projections at the isolated bridge', async () => {
    const invoke = vi
      .fn()
      .mockResolvedValueOnce([
        {
          profileId: 'deepseek_v4_1_flash_chat',
          profileVersion: 1,
          displayName: 'DeepSeek',
          compatibleDialects: ['openai_chat_completions'],
          settingsKind: 'none',
          selectable: false,
          credentialRef: 'private-reference'
        }
      ])
      .mockResolvedValueOnce([
        {
          vendorId: 'moonshot',
          displayName: 'Moonshot AI',
          selectable: true,
          apiToken: 'must-not-cross-the-boundary'
        }
      ])
      .mockResolvedValueOnce({
        status: 'unsupported',
        vendorId: 'moonshot',
        reason: 'unsupported_model',
        credential_ref: 'private-reference'
      })
    const bridge = createStorageIpcBridge({ invoke } as unknown as Pick<
      IpcRenderer,
      'invoke' | 'on' | 'removeListener'
    >)

    await expect(bridge.loadProviderProfileUiDescriptors()).rejects.toThrow(
      /credential field credentialRef/
    )
    await expect(bridge.loadProviderVendorDescriptors()).rejects.toThrow(
      /unexpected field apiToken/
    )
    await expect(
      bridge.resolveProviderVendorModelPolicy({
        vendorId: 'moonshot',
        modelId: 'unknown',
        dialect: 'openai_chat_completions'
      })
    ).rejects.toThrow(/unexpected field credential_ref/)
  })

  it('rejects legacy secret fields before they can cross the isolated bridge', async () => {
    const invoke = vi.fn().mockResolvedValue({
      apiUrl: '',
      apiTokenStatus: 'configured',
      apiToken: 'must-not-cross-the-boundary',
      searchMode: 'auto',
      tavilyApiKeyStatus: 'missing',
      models: []
    })
    const bridge = createStorageIpcBridge({ invoke } as unknown as Pick<
      IpcRenderer,
      'invoke' | 'on' | 'removeListener'
    >)

    await expect(bridge.loadModelSettings()).rejects.toThrow(/credential field apiToken/)
    await expect(
      bridge.saveModelSettings({
        expectedRevision: null,
        apiUrl: '',
        apiTokenMutation: { type: 'replace', value: 'contains whitespace' },
        searchMode: 'auto',
        tavilyApiKeyMutation: { type: 'keep' },
        models: []
      })
    ).rejects.toThrow(/apiTokenMutation/)
    expect(invoke).toHaveBeenCalledOnce()
  })

  it('strictly parses a successful save response before returning it to Renderer', async () => {
    const invoke = vi.fn().mockResolvedValue({
      ok: true,
      value: {
        apiUrl: '',
        apiTokenStatus: 'configured',
        apiToken: 'must-not-cross-the-boundary',
        searchMode: 'auto',
        tavilyApiKeyStatus: 'missing',
        models: []
      }
    })
    const bridge = createStorageIpcBridge({ invoke } as unknown as Pick<
      IpcRenderer,
      'invoke' | 'on' | 'removeListener'
    >)

    await expect(
      bridge.saveModelSettings({
        expectedRevision: null,
        apiUrl: '',
        apiTokenMutation: { type: 'keep' },
        searchMode: 'auto',
        tavilyApiKeyMutation: { type: 'keep' },
        models: []
      })
    ).rejects.toThrow(/credential field apiToken/)
  })

  it('passes the structured conversation-fork result through without throwing', async () => {
    const response = {
      ok: false,
      error: {
        message: 'Conversation fork was rejected.',
        code: -32000,
        data: {
          type: 'conversation_fork',
          code: 'active_command_session',
          conversationId: 'conversation-1',
          activeSessionCount: 1
        }
      }
    } satisfies HostInvocationResult<StorageChatConversationRecord>
    const invoke = vi.fn().mockResolvedValue(response)
    const bridge = createStorageIpcBridge({ invoke } as unknown as Pick<
      IpcRenderer,
      'invoke' | 'on' | 'removeListener'
    >)
    const input = {
      requestId: 'conversation-fork-request-1',
      sourceConversationId: 'conversation-1',
      forkPoint: { kind: 'assistant_reply', assistantMessageId: 'assistant-1' } as const
    }

    await expect(bridge.forkConversation(input)).resolves.toBe(response)
    expect(invoke).toHaveBeenCalledWith('host:storage.forkConversation', input)
  })

  it('routes lightweight Composer text autosaves without the full draft payload', async () => {
    const invoke = vi.fn().mockResolvedValue(true)
    const bridge = createStorageIpcBridge({ invoke } as unknown as Pick<
      IpcRenderer,
      'invoke' | 'on' | 'removeListener'
    >)
    const input = { scopeId: 'conversation-1', message: 'latest text', updatedAt: 42 }

    await expect(bridge.saveComposerDraftMessage(input)).resolves.toBe(true)
    expect(invoke).toHaveBeenCalledWith('host:storage.saveComposerDraftMessage', input)
  })
})

describe('human answer proof at the isolated storage bridge', () => {
  const response = {
    type: 'human_interaction_response',
    schemaVersion: 1,
    requestId: 'request',
    responseId: 'response',
    answers: [{ kind: 'text', questionId: 'q', question: 'Which?', answer: 'Blue' }]
  }
  const message = {
    id: 'answer',
    role: 'user',
    content: JSON.stringify(response),
    createdAt: 1,
    humanInteractionResponse: response
  }
  it('preserves verified output and rejects a malformed or foreign display receipt', async () => {
    const value = { id: 'chat', title: 'chat', createdAt: 1, updatedAt: 1, messages: [message] }
    const invoke = vi.fn().mockResolvedValue(value)
    const bridge = createStorageIpcBridge({ invoke } as unknown as Pick<
      IpcRenderer,
      'invoke' | 'on' | 'removeListener'
    >)
    await expect(bridge.loadConversation('chat')).resolves.toEqual(value)
    invoke.mockResolvedValue({
      ...value,
      messages: [{ ...message, humanInteractionResponse: { ...response, responseId: 'foreign' } }]
    })
    await expect(bridge.loadConversation('chat')).rejects.toThrow('complete User content')
  })
  it('refuses caller-made proof on write before entering IPC', () => {
    const invoke = vi.fn()
    const bridge = createStorageIpcBridge({ invoke } as unknown as Pick<
      IpcRenderer,
      'invoke' | 'on' | 'removeListener'
    >)
    for (const key of ['humanInteractionResponse', 'humanInteractionDisplay']) {
      const message = { id: 'message', content: '{}', role: 'user', createdAt: 1, [key]: response }
      expect(() =>
        bridge.upsertChatMessages({
          conversationId: 'chat',
          messages: [message],
          positionOffset: 0
        })
      ).toThrow('read-only')
      expect(() => bridge.saveChatMessageState({ conversationId: 'chat', message })).toThrow(
        'read-only'
      )
      expect(() => bridge.saveChatMessageUiState({ conversationId: 'chat', message })).toThrow(
        'read-only'
      )
    }
    expect(invoke).not.toHaveBeenCalled()
  })
})
