import { unwrapHostInvocation } from '@mycopilot/host-api'
import {
  IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
  parseAgentCollaborationSettings,
  parseHumanInteractionSettings,
  parseImageGenerationGetConfigurationOutput,
  parseImageGenerationSetEnabledOutput
} from '@mycopilot/protocol'
import { ArrowLeft, Bot, Globe, Image, Leaf, Monitor, Plug, Search, Users } from 'lucide-react'
import { useEffect, useId, useRef, useState, type ComponentType } from 'react'
import { ConfirmationDialog } from '../../components/dialog/ConfirmationDialog'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { useModelSettings } from '../../config/ModelSettingsProvider'
import { hostClient } from '../../host/hostClient'
import { getImageGenerationConfigurationErrorDetails } from '../imageGeneration/configuration/imageGenerationErrors'
import { getMcpManagementErrorDetails } from '../mcp/mcpManagementErrors'
import { useBuiltinMcpCapabilities } from '../mcp/useBuiltinMcpCapabilities'
import { useMcpManagement } from '../mcp/useMcpManagement'
import { loadAgentPromptPreferences, saveAgentPromptPreferences } from '../storage/storageClient'
import { useCapabilitySnapshot } from './useCapabilitySnapshot'
import './CapabilityCenterMenu.css'

const imageSource = {
  load: async () =>
    parseImageGenerationGetConfigurationOutput(
      unwrapHostInvocation(await hostClient.imageGeneration.getConfiguration())
    ).configuration,
  subscribe: (changed: () => void) => hostClient.imageGeneration.onChanged(changed)
}
const humanSource = {
  load: async () =>
    parseHumanInteractionSettings(
      unwrapHostInvocation(await hostClient.humanInteraction.getSettings({}))
    ),
  subscribe: (changed: () => void) => {
    const offChanged = hostClient.humanInteraction.onSettingsChanged(changed)
    const offResync = hostClient.humanInteraction.onResync(changed)
    return () => {
      offChanged()
      offResync()
    }
  },
  revision: (value: { revision: number }) => value.revision
}
const collaborationSource = {
  load: async () =>
    parseAgentCollaborationSettings(
      unwrapHostInvocation(await hostClient.agent.getCollaborationSettings({}))
    ),
  subscribe: (changed: () => void) => hostClient.agent.onCollaborationSettingsChanged(changed),
  revision: (value: { revision: number }) => value.revision
}
const promptPreferencesSource = {
  load: loadAgentPromptPreferences,
  subscribe: (changed: () => void) => hostClient.agent.onPromptPreferencesChanged(changed),
  revision: (value: { updatedAt: number }) => value.updatedAt
}

interface CapabilityRow {
  id: string
  label: string
  keywords: string
  icon: ComponentType<{ 'aria-hidden'?: boolean }>
  enabled: boolean
  disabled: boolean
  toggle: () => Promise<void>
}

export interface CapabilityCenterMenuProps {
  onBack: () => void
  onDialogOpenChange?: (open: boolean) => void
}

export function CapabilityCenterMenu({ onBack, onDialogOpenChange }: CapabilityCenterMenuProps) {
  const { t } = useFrontendConfig()
  const model = useModelSettings()
  const image = useCapabilitySnapshot(imageSource)
  const human = useCapabilitySnapshot(humanSource)
  const collaboration = useCapabilitySnapshot(collaborationSource)
  const promptPreferences = useCapabilitySnapshot(promptPreferencesSource)
  const builtin = useBuiltinMcpCapabilities()
  const mcp = useMcpManagement()
  const [query, setQuery] = useState('')
  const [selectedId, setSelectedId] = useState('lightweight')
  const [pending, setPending] = useState<ReadonlySet<string>>(() => new Set())
  const pendingRef = useRef(new Set<string>())
  const [error, setError] = useState<string | null>(null)
  const menuRef = useRef<HTMLElement>(null)
  const searchRef = useRef<HTMLInputElement>(null)
  const listRef = useRef<HTMLDivElement>(null)
  const mounted = useRef(true)
  const composing = useRef(false)
  const compositionEndedAt = useRef(-Infinity)
  const listId = useId()

  useEffect(() => {
    mounted.current = true
    menuRef.current?.focus({ preventScroll: true })
    return () => {
      mounted.current = false
    }
  }, [])
  useEffect(() => {
    onDialogOpenChange?.(error !== null)
    return () => onDialogOpenChange?.(false)
  }, [error, onDialogOpenChange])

  const showFailure = (cause: unknown) => {
    const imageError = getImageGenerationConfigurationErrorDetails(cause)
    const mcpError = getMcpManagementErrorDetails(cause)
    if (imageError.recovery === 'fixConfiguration' || imageError.recovery === 'reenterCredential') {
      setError(t('capabilityCenter.imageNotConfigured'))
    } else if (
      mcpError.code === 'authorizationRequired' ||
      mcpError.code === 'authorizationStale'
    ) {
      setError(t('capabilityCenter.mcpAuthorizationRequired'))
    } else {
      setError(mcpError.message || t('capabilityCenter.saveFailed'))
    }
  }

  useEffect(() => {
    if (
      image.error ||
      human.error ||
      collaboration.error ||
      promptPreferences.error ||
      builtin.state.status === 'error' ||
      mcp.state.status === 'error'
    ) {
      setError(t('capabilityCenter.loadFailed'))
    }
  }, [
    image.error,
    human.error,
    collaboration.error,
    promptPreferences.error,
    builtin.state.status,
    mcp.state.status,
    t
  ])

  const browser = builtin.state.output?.capabilities.find(
    (item) => item.capabilityId === 'browser_automation'
  )
  const software: CapabilityRow[] = [
    {
      id: 'lightweight',
      label: t('personalization.minimalMode'),
      keywords: 'lightweight minimal mode 轻量模式 輕量模式',
      icon: Leaf,
      enabled: promptPreferences.value?.contextProfile === 'minimal',
      disabled: !promptPreferences.available || promptPreferences.pending,
      toggle: async () => {
        if (!promptPreferences.value) return
        const contextProfile =
          promptPreferences.value.contextProfile === 'minimal' ? 'full' : 'minimal'
        await promptPreferences.mutate(async () => {
          // The storage API writes the whole record. Read immediately before saving so this
          // shortcut preserves the latest personalization settings and saved instructions.
          const latest = await loadAgentPromptPreferences()
          await saveAgentPromptPreferences({ ...latest, contextProfile })
        })
      }
    },
    {
      id: 'image',
      label: t('capabilityCenter.image'),
      keywords: 'image generation 图片生成',
      icon: Image,
      enabled: image.value?.enabled ?? false,
      disabled: !image.available || image.pending,
      toggle: async () => {
        const configuration = image.value
        if (!configuration) return
        if (
          !configuration.enabled &&
          (!configuration.endpointUrl ||
            !configuration.modelId ||
            configuration.credentialStatus !== 'configured')
        ) {
          setError(t('capabilityCenter.imageNotConfigured'))
          return
        }
        await image.mutate(async () =>
          parseImageGenerationSetEnabledOutput(
            unwrapHostInvocation(
              await hostClient.imageGeneration.setEnabled({
                schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
                expectedRevision: configuration.revision,
                enabled: !configuration.enabled
              })
            )
          )
        )
      }
    },
    {
      id: 'search',
      label: t('capabilityCenter.webSearch'),
      keywords: 'web search 联网搜索',
      icon: Globe,
      enabled: model.searchMode !== 'disabled' && model.tavilyApiKeyStatus === 'configured',
      disabled: false,
      toggle: async () => {
        if (model.tavilyApiKeyStatus !== 'configured') {
          setError(t('capabilityCenter.searchNotConfigured'))
          return
        }
        await model.saveSearchMode(model.searchMode === 'disabled' ? 'auto' : 'disabled')
      }
    },
    {
      id: 'human',
      label: t('capabilityCenter.human'),
      keywords: 'human interaction 人机交互',
      icon: Bot,
      enabled: human.value?.enabled ?? false,
      disabled: !human.available || human.pending,
      toggle: async () => {
        if (!human.value) return
        const { enabled, revision } = human.value
        await human.mutate(async () =>
          parseHumanInteractionSettings(
            unwrapHostInvocation(
              await hostClient.humanInteraction.updateSettings({
                enabled: !enabled,
                expectedRevision: revision
              })
            )
          )
        )
      }
    },
    {
      id: 'collaboration',
      label: t('capabilityCenter.collaboration'),
      keywords: 'multi agent 多智能体',
      icon: Users,
      enabled: collaboration.value?.enabled ?? false,
      disabled: !collaboration.available || collaboration.pending,
      toggle: async () => {
        if (!collaboration.value) return
        const { enabled, revision } = collaboration.value
        await collaboration.mutate(async () =>
          parseAgentCollaborationSettings(
            unwrapHostInvocation(
              await hostClient.agent.updateCollaborationSettings({
                enabled: !enabled,
                expectedRevision: revision
              })
            )
          )
        )
      }
    },
    {
      id: 'browser',
      label: t('capabilityCenter.browser'),
      keywords: 'browser automation 浏览器自动化',
      icon: Monitor,
      enabled: browser?.userAllowed ?? false,
      disabled: !browser || builtin.pendingCapabilities.has('browser_automation'),
      toggle: async () => {
        if (!browser) return
        if (!(await builtin.setAllowed(browser, !browser.userAllowed)))
          throw new Error('Capability changed; refresh required.')
      }
    }
  ]
  const servers: CapabilityRow[] = (mcp.state.output?.servers ?? []).map((server) => ({
    id: `mcp:${server.serverId}`,
    label: server.displayName,
    keywords: `mcp ${server.displayName}`,
    icon: Plug,
    enabled: server.enabled,
    disabled: mcp.pendingOperations.has(server.serverId),
    toggle: async () => {
      if (!server.enabled && server.launchAuthorizationState !== 'authorized') {
        setError(t('capabilityCenter.mcpAuthorizationRequired'))
        return
      }
      if (server.enabled) {
        if (!(await mcp.disableServer(server))) throw new Error('MCP state requires refresh.')
      } else {
        const enabled = await mcp.enableServer(server)
        if (!enabled) throw new Error('MCP state requires refresh.')
        if (!(await mcp.startServer(enabled))) throw new Error('MCP state requires refresh.')
      }
    }
  }))
  const match = (row: CapabilityRow) =>
    `${row.label} ${row.keywords}`.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase())
  const softwareRows = software.filter(match)
  const serverRows = servers.filter(match)
  const rows = [...softwareRows, ...serverRows]
  const selected = rows.find((row) => row.id === selectedId) ?? rows[0]

  useEffect(() => {
    const item = listRef.current?.querySelector<HTMLElement>('[data-selected="true"]')
    const list = listRef.current
    if (!item || !list) return
    const bounds = item.getBoundingClientRect()
    const container = list.getBoundingClientRect()
    if (bounds.top < container.top) list.scrollTop -= container.top - bounds.top
    else if (bounds.bottom > container.bottom) list.scrollTop += bounds.bottom - container.bottom
  }, [selected?.id])

  const toggle = async (row: CapabilityRow) => {
    if (row.disabled || pendingRef.current.has(row.id)) return
    pendingRef.current.add(row.id)
    setPending(new Set(pendingRef.current))
    try {
      await row.toggle()
    } catch (cause) {
      if (row.id.startsWith('mcp:')) await mcp.refresh()
      if (mounted.current) showFailure(cause)
    } finally {
      pendingRef.current.delete(row.id)
      if (mounted.current) setPending(new Set(pendingRef.current))
    }
  }

  const renderRow = (row: CapabilityRow) => {
    const Icon = row.icon
    const disabled = row.disabled || pending.has(row.id)
    return (
      <button
        key={row.id}
        type="button"
        role="switch"
        aria-checked={row.enabled}
        aria-label={row.label}
        aria-disabled={disabled}
        data-selected={selected?.id === row.id}
        className="capability-center__row"
        onMouseEnter={() => setSelectedId(row.id)}
        onFocus={() => setSelectedId(row.id)}
        onClick={() => void toggle(row)}
      >
        <Icon aria-hidden={true} />
        <span className="capability-center__label">{row.label}</span>
        <span
          className="settings-switch capability-center__toggle"
          data-state={row.enabled ? 'on' : 'off'}
          aria-hidden="true"
        >
          <span className="settings-switch__thumb" />
        </span>
      </button>
    )
  }

  return (
    <section
      ref={menuRef}
      className="capability-center"
      aria-label={t('capabilityCenter.title')}
      tabIndex={-1}
      onCompositionStart={() => {
        composing.current = true
      }}
      onCompositionEnd={(event) => {
        composing.current = false
        compositionEndedAt.current = event.timeStamp
      }}
      onKeyDown={(event) => {
        // Portal events also bubble through this section. The dialog owns its focus trap.
        if (error) return
        if (
          composing.current ||
          event.nativeEvent.isComposing ||
          event.keyCode === 229 ||
          event.timeStamp - compositionEndedAt.current < 120
        ) {
          event.stopPropagation()
          return
        }
        if (event.key === 'Escape') return // The composer owns the submenu's back action.
        event.stopPropagation()
        if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
          event.preventDefault()
          if (!rows.length) return
          const index = Math.max(
            0,
            rows.findIndex((row) => row.id === selected?.id)
          )
          setSelectedId(
            rows[(index + (event.key === 'ArrowDown' ? 1 : -1) + rows.length) % rows.length].id
          )
        } else if (
          event.key === 'Enter' &&
          (event.target === menuRef.current ||
            event.target === searchRef.current ||
            (event.target instanceof HTMLElement &&
              event.target.closest('.capability-center__row')))
        ) {
          event.preventDefault()
          if (selected) void toggle(selected)
        }
      }}
    >
      <header className="capability-center__header">
        <button
          type="button"
          className="capability-center__back"
          aria-label={t('capabilityCenter.back')}
          onClick={onBack}
        >
          <ArrowLeft aria-hidden="true" />
        </button>
        <span>{t('capabilityCenter.title')}</span>
        <label className="capability-center__search">
          <Search aria-hidden="true" />
          <input
            ref={searchRef}
            type="search"
            value={query}
            aria-label={t('capabilityCenter.search')}
            placeholder={t('capabilityCenter.search')}
            aria-controls={listId}
            onChange={(event) => setQuery(event.target.value)}
          />
        </label>
      </header>
      <div ref={listRef} id={listId} className="capability-center__list">
        {softwareRows.map(renderRow)}
        {serverRows.length > 0 && <div className="capability-center__heading">MCP</div>}
        {serverRows.map(renderRow)}
        {mcp.state.status === 'ready' && servers.length === 0 && !query && (
          <>
            <div className="capability-center__heading">MCP</div>
            <div className="capability-center__empty">{t('capabilityCenter.noMcp')}</div>
          </>
        )}
        {!rows.length && (
          <div className="capability-center__empty">{t('capabilityCenter.noResults')}</div>
        )}
      </div>
      {error && (
        <ConfirmationDialog
          title={t('capabilityCenter.notice')}
          description={error}
          cancelLabel={t('imagePreview.close')}
          confirmLabel={t('capabilityCenter.acknowledge')}
          confirmVariant="primary"
          showCancelButton={false}
          fallbackFocusRef={menuRef}
          onCancel={() => setError(null)}
          onConfirm={() => setError(null)}
        />
      )}
    </section>
  )
}
