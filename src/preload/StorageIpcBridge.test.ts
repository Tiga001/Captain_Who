import type { IpcRenderer } from 'electron'
import type { HostInvocationResult } from '@mycopilot/host-api'
import type { StorageChatConversationRecord } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { createStorageIpcBridge } from './StorageIpcBridge'

describe('Storage IPC bridge', () => {
  it('routes Provider Profile descriptors and authoritative model saves over dedicated channels', async () => {
    const descriptors = [
      {
        profileId: 'deepseek_v4_chat',
        profileVersion: 1,
        displayName: '深度求索 / DeepSeek（V4 Chat）',
        compatibleDialects: ['openai_chat_completions'],
        settingsKind: 'deepseek_v4_chat',
        selectable: true
      }
    ]
    const authoritativeSettings = {
      apiUrl: '',
      apiToken: '',
      searchMode: 'auto',
      tavilyApiKey: '',
      models: []
    }
    const invoke = vi
      .fn()
      .mockResolvedValueOnce(descriptors)
      .mockResolvedValueOnce({ ok: true, value: authoritativeSettings })
    const bridge = createStorageIpcBridge({ invoke } as unknown as Pick<IpcRenderer, 'invoke'>)

    await expect(bridge.loadProviderProfileUiDescriptors()).resolves.toBe(descriptors)
    await expect(bridge.saveModelSettings(authoritativeSettings)).resolves.toEqual({
      ok: true,
      value: authoritativeSettings
    })
    expect(invoke).toHaveBeenNthCalledWith(1, 'host:storage.loadProviderProfileUiDescriptors')
    expect(invoke).toHaveBeenNthCalledWith(
      2,
      'host:storage.saveModelSettings',
      authoritativeSettings
    )
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
    const bridge = createStorageIpcBridge({ invoke } as unknown as Pick<IpcRenderer, 'invoke'>)

    await expect(bridge.loadProviderVendorDescriptors()).resolves.toBe(descriptors)
    await expect(bridge.resolveProviderVendorModelPolicy(input)).resolves.toBe(policy)
    expect(invoke).toHaveBeenNthCalledWith(1, 'host:storage.loadProviderVendorDescriptors')
    expect(invoke).toHaveBeenNthCalledWith(
      2,
      'host:storage.resolveProviderVendorModelPolicy',
      input
    )
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
    const bridge = createStorageIpcBridge({ invoke } as unknown as Pick<IpcRenderer, 'invoke'>)
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
    const bridge = createStorageIpcBridge({ invoke } as unknown as Pick<IpcRenderer, 'invoke'>)
    const input = { scopeId: 'conversation-1', message: 'latest text', updatedAt: 42 }

    await expect(bridge.saveComposerDraftMessage(input)).resolves.toBe(true)
    expect(invoke).toHaveBeenCalledWith('host:storage.saveComposerDraftMessage', input)
  })
})
