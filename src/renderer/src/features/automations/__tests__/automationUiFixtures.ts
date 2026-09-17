import type { AutomationDraft } from '../automationTypes'
import type { AutomationRun, AutomationTask } from '@mycopilot/protocol'
import type { ModelConfig } from '../../../config/modelConfig'
import type { ChatConversation } from '../../chat/chatTypes'

export const testModel: ModelConfig = {
  id: 'model-1',
  providerModelId: 'provider-model-1',
  displayName: 'Test Model',
  apiTokenOverrideStatus: 'missing',
  apiTokenOverrideMutation: { type: 'keep' },
  supportsImage: true,
  inputPrice: '0',
  cachedInputPrice: '',
  outputPrice: '0',
  enabled: true,
  execution: { status: 'available' },
  providerProfileConfig: {
    schemaVersion: 1,
    profile: { id: 'generic_openai_chat', version: 1 },
    reasoning: { mode: 'enabled', effort: 'high' }
  },
  providerProfileUpdate: { kind: 'unchanged' }
}

export const testConversation: ChatConversation = {
  id: 'conversation-1',
  projectId: 'project-1',
  modelId: 'model-1',
  title: 'Automation chat',
  messages: [],
  createdAt: 1_000,
  updatedAt: 2_000
}

export function makeAutomationDraft(patch: Partial<AutomationDraft> = {}): AutomationDraft {
  return {
    title: 'Daily brief',
    prompt: 'Summarize the important updates.',
    status: 'active',
    destination: {
      kind: 'new_chat',
      projectBinding: 'project',
      projectId: 'project-1',
      modelId: 'model-1'
    },
    permissionMode: 'default',
    permissionModeVersion: 2,
    schedule: {
      kind: 'daily',
      timeMinutes: 9 * 60,
      anchorAt: 1_800_000_000_000,
      timezone: 'Asia/Shanghai'
    },
    notificationPolicy: 'all_runs',
    ...patch
  }
}

export function makeAutomationRun(patch: Partial<AutomationRun> = {}): AutomationRun {
  return {
    schemaVersion: 1,
    runId: 'run-1',
    automationId: 'automation-1',
    configRevision: 1,
    triggerKind: 'manual',
    scheduledFor: null,
    status: 'completed',
    conversationId: 'conversation-1',
    userMessageId: 'message-user-1',
    assistantMessageId: 'message-assistant-1',
    reportKind: 'completed',
    resultPreview: 'Done',
    errorCode: null,
    errorMessage: null,
    attention: null,
    createdAt: 1_800_000_000_000,
    startedAt: 1_800_000_000_010,
    completedAt: 1_800_000_000_020,
    updatedAt: 1_800_000_000_020,
    ...patch
  }
}

export function makeAutomationTask(patch: Partial<AutomationTask> = {}): AutomationTask {
  return {
    schemaVersion: 1,
    automationId: 'automation-1',
    title: 'Daily brief',
    prompt: 'Summarize the important updates.',
    status: 'active',
    health: { state: 'ok' },
    destination: {
      kind: 'new_chat',
      projectBinding: 'project',
      projectId: 'project-1',
      modelId: 'model-1',
      reasoning: { source: 'model_config', mode: 'enabled', effort: 'high' }
    },
    permissionMode: 'default',
    permissionModeVersion: 2,
    resolvedPermissions: {
      read: 'workspace_only',
      write: 'workspace_only',
      command: 'require_approval',
      commandSafety: 'guarded',
      patch: 'require_approval',
      builtinExecution: 'require_approval'
    },
    schedule: {
      kind: 'daily',
      timeMinutes: 9 * 60,
      anchorAt: 1_800_000_000_000,
      timezone: 'Asia/Shanghai'
    },
    scheduleSummary: 'Daily at 09:00',
    rrule: 'FREQ=DAILY',
    timezone: 'Asia/Shanghai',
    notificationPolicy: 'all_runs',
    targetSnapshot: {
      projectName: 'Project One',
      conversationTitle: null,
      modelDisplayName: 'Test Model'
    },
    nextRunAt: 1_800_000_000_000,
    lastScheduledAt: null,
    lastRunAt: null,
    latestRun: null,
    attention: null,
    revision: 1,
    createdAt: 1_700_000_000_000,
    updatedAt: 1_700_000_000_000,
    ...patch
  }
}
