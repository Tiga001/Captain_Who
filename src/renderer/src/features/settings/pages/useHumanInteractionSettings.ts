import { useCallback, useEffect, useRef, useState } from 'react'
import { unwrapHostInvocation } from '@mycopilot/host-api'
import { parseHumanInteractionSettings, type HumanInteractionSettings } from '@mycopilot/protocol'
import { hostClient } from '../../../host/hostClient'

export function useHumanInteractionSettings() {
  const [settings, setSettings] = useState<HumanInteractionSettings | null>(null)
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<'load' | 'save' | null>(null)
  const current = useRef<HumanInteractionSettings | null>(null)
  const lifecycle = useRef(0)
  const readSequence = useRef(0)
  const savePending = useRef(false)

  const accept = useCallback((next: HumanInteractionSettings) => {
    if (current.current && next.revision <= current.current.revision) return
    current.current = next
    setSettings(next)
  }, [])

  const refresh = useCallback(async () => {
    if (savePending.current) return
    const generation = lifecycle.current
    const sequence = ++readSequence.current
    setLoading(true)
    try {
      const next = parseHumanInteractionSettings(
        unwrapHostInvocation(await hostClient.humanInteraction.getSettings({}))
      )
      if (lifecycle.current !== generation || readSequence.current !== sequence) return
      accept(next)
      setError(null)
    } catch {
      if (lifecycle.current === generation && readSequence.current === sequence) setError('load')
    } finally {
      if (lifecycle.current === generation && readSequence.current === sequence) setLoading(false)
    }
  }, [accept])

  useEffect(() => {
    const generation = ++lifecycle.current
    let unsubscribe: (() => void) | undefined
    let unsubscribeResync: (() => void) | undefined
    try {
      unsubscribe = hostClient.humanInteraction.onSettingsChanged((value) => {
        if (lifecycle.current !== generation) return
        try {
          const next = parseHumanInteractionSettings(value)
          if (current.current && next.revision < current.current.revision) return
          readSequence.current += 1
          accept(next)
          setLoading(false)
          setError(null)
        } catch {
          setError('load')
        }
      })
      unsubscribeResync = hostClient.humanInteraction.onResync(() => void refresh())
    } catch {
      setError('load')
    }
    void refresh()
    const onFocus = () => void refresh()
    window.addEventListener('focus', onFocus)
    window.addEventListener('online', onFocus)
    return () => {
      lifecycle.current += 1
      unsubscribe?.()
      unsubscribeResync?.()
      window.removeEventListener('focus', onFocus)
      window.removeEventListener('online', onFocus)
    }
  }, [accept, refresh])

  const toggle = useCallback(async () => {
    const previous = current.current
    if (!previous || savePending.current || loading) return
    const generation = lifecycle.current
    savePending.current = true
    readSequence.current += 1
    setSaving(true)
    setError(null)
    try {
      const next = parseHumanInteractionSettings(
        unwrapHostInvocation(
          await hostClient.humanInteraction.updateSettings({
            enabled: !previous.enabled,
            expectedRevision: previous.revision
          })
        )
      )
      if (lifecycle.current !== generation) return
      accept(next)
    } catch {
      if (lifecycle.current === generation) setError('save')
    } finally {
      savePending.current = false
      if (lifecycle.current === generation) setSaving(false)
    }
  }, [accept, loading])

  return { settings, loading, saving, error, refresh, toggle }
}
