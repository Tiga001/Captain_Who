import { useCallback, useEffect, useRef, useState } from 'react'

interface CapabilitySource<T> {
  load: () => Promise<T>
  subscribe: (changed: () => void) => () => void
  revision?: (value: T) => number
}

/** Domain-owned settings stay authoritative; notifications only invalidate cached reads. */
export function useCapabilitySnapshot<T>(source: CapabilitySource<T>) {
  const [value, setValue] = useState<T | null>(null)
  const [available, setAvailable] = useState(false)
  const [error, setError] = useState<unknown>(null)
  const [pending, setPending] = useState(false)
  const current = useRef<T | null>(null)
  const lifecycle = useRef(0)
  const sequence = useRef(0)
  const mutating = useRef(false)
  const invalidatedDuringMutation = useRef(false)

  const refresh = useCallback(async () => {
    const generation = lifecycle.current
    const request = ++sequence.current
    try {
      const next = await source.load()
      if (generation !== lifecycle.current || request !== sequence.current) return
      if (
        current.current &&
        source.revision &&
        source.revision(next) < source.revision(current.current)
      )
        return
      current.current = next
      setValue(next)
      setAvailable(true)
      setError(null)
    } catch (cause) {
      if (generation !== lifecycle.current || request !== sequence.current) return
      setAvailable(false)
      setError(cause)
      throw cause
    }
  }, [source])

  useEffect(() => {
    lifecycle.current += 1
    const reload = () => {
      if (mutating.current) {
        invalidatedDuringMutation.current = true
        sequence.current += 1
      } else void refresh().catch(() => undefined)
    }
    const unsubscribe = source.subscribe(reload)
    reload()
    window.addEventListener('focus', reload)
    window.addEventListener('online', reload)
    const onVisibility = () => {
      if (document.visibilityState === 'visible') reload()
    }
    document.addEventListener('visibilitychange', onVisibility)
    return () => {
      lifecycle.current += 1
      sequence.current += 1
      unsubscribe()
      window.removeEventListener('focus', reload)
      window.removeEventListener('online', reload)
      document.removeEventListener('visibilitychange', onVisibility)
    }
  }, [refresh, source])

  const mutate = useCallback(
    async (operation: () => Promise<unknown>) => {
      if (mutating.current) return
      const generation = lifecycle.current
      mutating.current = true
      sequence.current += 1
      setPending(true)
      const reconcile = async () => {
        do {
          invalidatedDuringMutation.current = false
          await refresh()
        } while (invalidatedDuringMutation.current && generation === lifecycle.current)
      }
      try {
        await operation()
        await reconcile()
      } catch (cause) {
        // Indeterminate commits and revision conflicts require a read, never a guessed rollback.
        await reconcile().catch(() => undefined)
        throw cause
      } finally {
        mutating.current = false
        if (generation === lifecycle.current) setPending(false)
      }
    },
    [refresh]
  )

  return { value, available, pending, error, mutate }
}
