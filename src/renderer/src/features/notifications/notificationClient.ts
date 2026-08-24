import {
  NOTIFICATION_SCHEMA_VERSION,
  type NotificationEvent,
  type NotificationMarkSeenInput,
  type NotificationMarkSeenOutput,
  type NotificationOpenRequest,
  type NotificationResync,
  type NotificationSettings,
  type NotificationSettingsGetOutput,
  type NotificationSettingsUpdateInput,
  type NotificationSettingsUpdateOutput
} from '@mycopilot/protocol'
import { HostInvocationError, unwrapHostInvocation } from '@mycopilot/host-api'
import { hostClient } from '../../host/hostClient'

export class NotificationClientError extends Error {
  readonly retryable: boolean

  constructor(message: string, retryable = true, options?: ErrorOptions) {
    super(message, options)
    this.name = 'NotificationClientError'
    this.retryable = retryable
  }
}

function notificationsApi(): Partial<typeof hostClient.notifications> | undefined {
  return (
    hostClient as unknown as {
      notifications?: Partial<typeof hostClient.notifications>
    }
  ).notifications
}

export function hasNotificationHostApi(): boolean {
  const api = notificationsApi()
  return Boolean(api?.getSettings && api.updateSettings && api.onEvent && api.onResync)
}

async function invoke<T>(operation: () => Promise<Parameters<typeof unwrapHostInvocation<T>>[0]>) {
  try {
    return unwrapHostInvocation(await operation())
  } catch (error) {
    if (error instanceof NotificationClientError) throw error
    if (error instanceof HostInvocationError) {
      throw new NotificationClientError(error.message, true, { cause: error })
    }
    throw new NotificationClientError(
      error instanceof Error ? error.message : String(error),
      true,
      {
        cause: error instanceof Error ? error : undefined
      }
    )
  }
}

export async function getNotificationSettings(): Promise<NotificationSettings> {
  const api = notificationsApi()
  if (!api?.getSettings) throw new NotificationClientError('Notification service is unavailable')
  const output: NotificationSettingsGetOutput = await invoke(() =>
    api.getSettings!({ schemaVersion: NOTIFICATION_SCHEMA_VERSION })
  )
  return output.settings
}

export async function markNotificationsSeen(
  target: NotificationMarkSeenInput['target']
): Promise<NotificationMarkSeenOutput> {
  const api = notificationsApi()
  if (!api?.markSeen) throw new NotificationClientError('Notification service is unavailable')
  return invoke(() => api.markSeen!({ schemaVersion: NOTIFICATION_SCHEMA_VERSION, target }))
}

export type NotificationSettingsDraft = Omit<
  NotificationSettings,
  'schemaVersion' | 'revision' | 'updatedAt'
>

export async function updateNotificationSettings(
  current: Pick<NotificationSettings, 'revision'>,
  settings: NotificationSettingsDraft
): Promise<NotificationSettings> {
  const api = notificationsApi()
  if (!api?.updateSettings) {
    throw new NotificationClientError('Notification service is unavailable')
  }
  const input: NotificationSettingsUpdateInput = {
    schemaVersion: NOTIFICATION_SCHEMA_VERSION,
    expectedRevision: current.revision,
    settings
  }
  const output: NotificationSettingsUpdateOutput = await invoke(() => api.updateSettings!(input))
  return output.settings
}

export function onNotificationEvent(handler: (event: NotificationEvent) => void): () => void {
  return notificationsApi()?.onEvent?.(handler) ?? (() => undefined)
}

export function onNotificationResync(handler: (event: NotificationResync) => void): () => void {
  return notificationsApi()?.onResync?.(handler) ?? (() => undefined)
}

export function onNotificationOpenRequested(
  handler: (request: NotificationOpenRequest) => void
): () => void {
  return notificationsApi()?.onOpenRequested?.(handler) ?? (() => undefined)
}
