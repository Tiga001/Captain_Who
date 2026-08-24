import { useCallback, useEffect, useRef, useState } from 'react'
import type { NotificationSettings } from '@mycopilot/protocol'
import {
  getNotificationSettings,
  hasNotificationHostApi,
  onNotificationEvent,
  onNotificationResync,
  updateNotificationSettings,
  type NotificationSettingsDraft
} from './notificationClient'

export type NotificationSettingsLoadStatus = 'idle' | 'loading' | 'ready' | 'error'

export function useNotificationSettings(enabled = true) {
  const effectiveEnabled = enabled && hasNotificationHostApi()
  const [settings, setSettings] = useState<NotificationSettings | null>(null)
  const [status, setStatus] = useState<NotificationSettingsLoadStatus>(
    effectiveEnabled ? 'loading' : 'ready'
  )
  const [error, setError] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)
  const savingRef = useRef<Promise<NotificationSettings> | null>(null)
  const requestRef = useRef(0)
  const refreshRef = useRef<() => Promise<void>>(async () => undefined)

  const refresh = useCallback(async () => {
    if (!effectiveEnabled) return
    const request = ++requestRef.current
    setStatus('loading')
    try {
      const next = await getNotificationSettings()
      if (request !== requestRef.current) return
      setSettings(next)
      setError(null)
      setStatus('ready')
    } catch (caught) {
      if (request !== requestRef.current) return
      setError(caught instanceof Error ? caught.message : String(caught))
      setStatus('error')
    }
  }, [effectiveEnabled])

  useEffect(() => {
    refreshRef.current = refresh
  }, [refresh])

  useEffect(() => {
    void refresh()
    if (!effectiveEnabled) {
      requestRef.current += 1
      return
    }

    const stopEvent = onNotificationEvent((event) => {
      if (event.kind === 'settings_updated') void refreshRef.current()
    })
    const stopResync = onNotificationResync(() => void refreshRef.current())
    return () => {
      requestRef.current += 1
      stopEvent()
      stopResync()
    }
  }, [effectiveEnabled, refresh])

  const update = useCallback(
    async (patch: Partial<NotificationSettingsDraft>) => {
      if (!settings) throw new Error('Notification settings are unavailable')
      if (savingRef.current) return savingRef.current
      const draft: NotificationSettingsDraft = {
        enabled: settings.enabled,
        soundEnabled: settings.soundEnabled,
        showTaskContent: settings.showTaskContent,
        humanCompletedEnabled: settings.humanCompletedEnabled,
        humanFailedEnabled: settings.humanFailedEnabled,
        humanApprovalEnabled: settings.humanApprovalEnabled,
        humanCancelledEnabled: settings.humanCancelledEnabled,
        ...patch
      }
      requestRef.current += 1
      const operation = updateNotificationSettings(settings, draft)
        .then((next) => {
          setSettings(next)
          setError(null)
          setStatus('ready')
          return next
        })
        .catch((caught) => {
          setError(caught instanceof Error ? caught.message : String(caught))
          setStatus('error')
          throw caught
        })
        .finally(() => {
          savingRef.current = null
          setSaving(false)
        })
      savingRef.current = operation
      setSaving(true)
      return operation
    },
    [settings]
  )

  return { settings, status, error, refresh, update, saving }
}
