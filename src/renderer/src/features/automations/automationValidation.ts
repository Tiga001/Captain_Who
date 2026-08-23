import type { AutomationDraft, AutomationValidationResult } from './automationTypes'
import { validateSchedule } from './automationSchedule'

const TITLE_MAX_BYTES = 512
const PROMPT_MAX_BYTES = 65_536

function utf8Bytes(value: string): number {
  return new TextEncoder().encode(value).byteLength
}

function notificationIsCompatible(draft: AutomationDraft): boolean {
  return draft.destination.kind === 'new_chat'
    ? draft.notificationPolicy === 'all_runs' || draft.notificationPolicy === 'unsuccessful_only'
    : draft.notificationPolicy === 'important_updates' ||
        draft.notificationPolicy === 'unsuccessful_only'
}

export function validateAutomationDraft(draft: AutomationDraft): AutomationValidationResult {
  const errors: AutomationValidationResult['errors'] = {}
  if (!draft.title.trim()) errors.title = 'title_required'
  else if (utf8Bytes(draft.title) > TITLE_MAX_BYTES) errors.title = 'title_too_long'

  if (!draft.prompt.trim()) errors.prompt = 'prompt_required'
  else if (utf8Bytes(draft.prompt) > PROMPT_MAX_BYTES) errors.prompt = 'prompt_too_long'

  if (draft.destination.kind === 'new_chat') {
    if (!draft.destination.modelId.trim()) errors.modelId = 'model_required'
    if (draft.destination.projectBinding === 'project' && !draft.destination.projectId?.trim()) {
      errors.projectId = 'project_required'
    }
    if (draft.destination.projectBinding === 'none' && draft.destination.projectId !== null) {
      errors.projectId = 'project_binding_invalid'
    }
  } else if (!draft.destination.conversationId.trim()) {
    errors.conversationId = 'conversation_required'
  }

  if (!['default', 'full', 'custom'].includes(draft.permissionMode)) {
    errors.permissionMode = 'permission_invalid'
  }
  if (!notificationIsCompatible(draft)) {
    errors.notificationPolicy = 'notification_incompatible'
  }

  Object.assign(errors, validateSchedule(draft.schedule).errors)
  return { valid: Object.keys(errors).length === 0, errors }
}

export function notificationPolicyForDestinationChange(
  current: AutomationDraft['notificationPolicy'],
  destinationKind: AutomationDraft['destination']['kind']
): AutomationDraft['notificationPolicy'] {
  if (destinationKind === 'new_chat') {
    return current === 'important_updates' ? 'all_runs' : current
  }
  return current === 'all_runs' ? 'important_updates' : current
}
