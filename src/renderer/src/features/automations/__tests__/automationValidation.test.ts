import { describe, expect, it } from 'vitest'
import type { AutomationDraft } from '../automationTypes'
import {
  notificationPolicyForDestinationChange,
  validateAutomationDraft
} from '../automationValidation'

function draft(overrides: Partial<AutomationDraft> = {}): AutomationDraft {
  return {
    title: 'Daily brief',
    prompt: 'Summarize the project status.',
    status: 'active',
    destination: {
      kind: 'new_chat',
      projectBinding: 'none',
      projectId: null,
      modelId: 'model-1'
    },
    permissionMode: 'default',
    permissionModeVersion: 2,
    schedule: {
      kind: 'daily',
      timeMinutes: 540,
      anchorAt: 1_777_777_777_000,
      timezone: 'Asia/Shanghai'
    },
    notificationPolicy: 'all_runs',
    ...overrides
  }
}

describe('automation draft validation', () => {
  it('accepts a complete new-chat task', () => {
    expect(validateAutomationDraft(draft())).toEqual({ valid: true, errors: {} })
  })

  it('locates missing title, prompt and model fields', () => {
    expect(
      validateAutomationDraft(
        draft({
          title: ' ',
          prompt: '',
          destination: {
            kind: 'new_chat',
            projectBinding: 'none',
            projectId: null,
            modelId: ''
          }
        })
      ).errors
    ).toMatchObject({
      title: 'title_required',
      prompt: 'prompt_required',
      modelId: 'model_required'
    })
  })

  it('requires an existing root conversation selection', () => {
    const result = validateAutomationDraft(
      draft({
        destination: { kind: 'existing_chat', conversationId: '' },
        notificationPolicy: 'important_updates'
      })
    )
    expect(result.errors.conversationId).toBe('conversation_required')
  })

  it('rejects a notification policy that does not belong to the destination', () => {
    expect(
      validateAutomationDraft(draft({ notificationPolicy: 'important_updates' })).errors
        .notificationPolicy
    ).toBe('notification_incompatible')
    expect(
      validateAutomationDraft(
        draft({
          destination: { kind: 'existing_chat', conversationId: 'conversation-1' },
          notificationPolicy: 'all_runs'
        })
      ).errors.notificationPolicy
    ).toBe('notification_incompatible')
  })

  it('maps notification defaults compatibly when switching destination', () => {
    expect(notificationPolicyForDestinationChange('all_runs', 'existing_chat')).toBe(
      'important_updates'
    )
    expect(notificationPolicyForDestinationChange('important_updates', 'new_chat')).toBe('all_runs')
    expect(notificationPolicyForDestinationChange('unsuccessful_only', 'new_chat')).toBe(
      'unsuccessful_only'
    )
  })
})
