import { useCallback, useEffect, useRef, useState } from 'react'
import {
  MCP_MANAGEMENT_SCHEMA_VERSION,
  type McpBuiltinCapabilityId,
  type McpBuiltinCapabilityListItem,
  type McpBuiltinCapabilityListOutput
} from '@mycopilot/protocol'
import { listMcpBuiltinCapabilities, setMcpBuiltinCapabilityAllowed } from './mcpManagementClient'
import { getMcpManagementErrorDetails } from './mcpManagementErrors'

export interface BuiltinMcpCapabilitiesViewState {
  errorMessage: string | null
  isRefreshing: boolean
  output: McpBuiltinCapabilityListOutput | null
  status: 'loading' | 'ready' | 'error'
}

const INITIAL_STATE: BuiltinMcpCapabilitiesViewState = {
  errorMessage: null,
  isRefreshing: false,
  output: null,
  status: 'loading'
}

/**
 * Renderer state for Host-managed capabilities.
 *
 * The existing `mcp.changed` contract is scoped to external Server identities, so this hook does
 * not reinterpret those notifications. It applies mutation responses authoritatively and safely
 * re-fetches when the Settings window regains focus or visibility.
 */
export function useBuiltinMcpCapabilities() {
  const [state, setState] = useState<BuiltinMcpCapabilitiesViewState>(INITIAL_STATE)
  const [pendingCapabilities, setPendingCapabilities] = useState<
    ReadonlySet<McpBuiltinCapabilityId>
  >(() => new Set())

  const mountedRef = useRef(false)
  const lifecycleEpochRef = useRef(0)
  const mutationEpochRef = useRef(0)
  const outputRef = useRef<McpBuiltinCapabilityListOutput | null>(null)
  const pendingRef = useRef<ReadonlySet<McpBuiltinCapabilityId>>(new Set())
  const refreshDirtyRef = useRef(false)
  const refreshVisibleRef = useRef(false)
  const refreshLoopRef = useRef<Promise<void> | null>(null)
  const refreshCallbackRef = useRef<() => Promise<McpBuiltinCapabilityListOutput | null>>(
    async () => null
  )

  const runRefreshLoop = useCallback(async () => {
    while (mountedRef.current && refreshDirtyRef.current) {
      refreshDirtyRef.current = false
      const showRefreshIndicator = refreshVisibleRef.current
      refreshVisibleRef.current = false
      const lifecycleEpoch = lifecycleEpochRef.current
      const mutationEpoch = mutationEpochRef.current

      setState((current) => ({
        ...current,
        errorMessage: current.output ? current.errorMessage : null,
        isRefreshing: Boolean(current.output) && showRefreshIndicator,
        status: current.output ? 'ready' : 'loading'
      }))

      try {
        const fetched = await listMcpBuiltinCapabilities()
        if (
          !mountedRef.current ||
          lifecycleEpoch !== lifecycleEpochRef.current ||
          mutationEpoch !== mutationEpochRef.current
        ) {
          continue
        }
        const current = outputRef.current
        if (current && fetched.revision < current.revision) continue
        outputRef.current = fetched
        setState({ errorMessage: null, isRefreshing: false, output: fetched, status: 'ready' })
      } catch (error) {
        if (!mountedRef.current || lifecycleEpoch !== lifecycleEpochRef.current) continue
        const message = getMcpManagementErrorDetails(error).message
        setState((current) =>
          current.output
            ? { ...current, errorMessage: message, isRefreshing: false, status: 'ready' }
            : { errorMessage: message, isRefreshing: false, output: null, status: 'error' }
        )
      }
    }
  }, [])

  const refresh = useCallback(
    async (showRefreshIndicator = false): Promise<McpBuiltinCapabilityListOutput | null> => {
      if (showRefreshIndicator) refreshVisibleRef.current = true
      refreshDirtyRef.current = true
      if (!refreshLoopRef.current) {
        const loop = runRefreshLoop().finally(() => {
          if (refreshLoopRef.current !== loop) return
          refreshLoopRef.current = null
          if (mountedRef.current && refreshDirtyRef.current) {
            queueMicrotask(() => {
              if (mountedRef.current && refreshDirtyRef.current) {
                void refreshCallbackRef.current()
              }
            })
          }
        })
        refreshLoopRef.current = loop
      }
      await refreshLoopRef.current
      return outputRef.current
    },
    [runRefreshLoop]
  )

  useEffect(() => {
    refreshCallbackRef.current = refresh
  }, [refresh])

  useEffect(() => {
    lifecycleEpochRef.current += 1
    mountedRef.current = true
    void refresh()

    const refreshOnFocus = () => void refresh()
    const refreshOnVisibility = () => {
      if (document.visibilityState === 'visible') void refresh()
    }
    window.addEventListener('focus', refreshOnFocus)
    document.addEventListener('visibilitychange', refreshOnVisibility)

    return () => {
      mountedRef.current = false
      lifecycleEpochRef.current += 1
      refreshDirtyRef.current = false
      refreshVisibleRef.current = false
      window.removeEventListener('focus', refreshOnFocus)
      document.removeEventListener('visibilitychange', refreshOnVisibility)
    }
  }, [refresh])

  const setAllowed = useCallback(
    async (
      capability: McpBuiltinCapabilityListItem,
      allowed: boolean
    ): Promise<McpBuiltinCapabilityListItem | null> => {
      if (pendingRef.current.has(capability.capabilityId)) return null
      pendingRef.current = new Set(pendingRef.current).add(capability.capabilityId)
      if (mountedRef.current) setPendingCapabilities(pendingRef.current)
      mutationEpochRef.current += 1

      try {
        const response = await setMcpBuiltinCapabilityAllowed({
          schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
          capabilityId: capability.capabilityId,
          allowed,
          expectedPolicyRevision: capability.policyRevision
        })
        if (!mountedRef.current) return null
        if (
          response.capability.capabilityId !== capability.capabilityId ||
          response.capability.userAllowed !== allowed ||
          response.capability.policyRevision < capability.policyRevision
        ) {
          void refresh()
          throw new Error('The built-in MCP capability response was not authoritative.')
        }

        const current = outputRef.current
        if (current && response.revision < current.revision) {
          void refresh()
          return null
        }
        const capabilities = current?.capabilities ?? []
        const nextCapabilities = capabilities.some(
          (item) => item.capabilityId === response.capability.capabilityId
        )
          ? capabilities.map((item) =>
              item.capabilityId === response.capability.capabilityId ? response.capability : item
            )
          : [...capabilities, response.capability]
        const output: McpBuiltinCapabilityListOutput = {
          schemaVersion: MCP_MANAGEMENT_SCHEMA_VERSION,
          revision: response.revision,
          capabilities: nextCapabilities
        }
        outputRef.current = output
        setState({ errorMessage: null, isRefreshing: false, output, status: 'ready' })
        return response.capability
      } catch (error) {
        await refresh()
        throw error
      } finally {
        const nextPending = new Set(pendingRef.current)
        nextPending.delete(capability.capabilityId)
        pendingRef.current = nextPending
        if (mountedRef.current) setPendingCapabilities(nextPending)
      }
    },
    [refresh]
  )

  return {
    pendingCapabilities,
    refresh,
    setAllowed,
    state
  }
}
