import { AlertTriangle, LoaderCircle, Plus, RefreshCw, Server } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { McpServerDetailsView, McpServerListItem } from '@mycopilot/protocol'
import { ConfirmationDialog } from '../../components/dialog/ConfirmationDialog'
import { useToast } from '../../components/toast/ToastContext'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { McpServerEditor } from './McpServerEditor'
import { McpServerList, McpServerListRefreshButton } from './McpServerList'
import {
  getMcpManagementErrorDetails,
  mcpOperationNeedsAuthoritativeConfirmation,
  mcpOperationNeedsLaunchAuthorization
} from './mcpManagementErrors'
import type { McpServerDraft } from './mcpManagementInputs'
import { toSafeMcpDisplayText } from './mcpSafeDisplay'
import { useMcpManagement } from './useMcpManagement'
import './McpSettingsPage.css'

type McpSettingsView = 'list' | 'add' | 'edit'

export interface McpSettingsPageProps {
  onDirtyChange?: (dirty: boolean) => void
}

export function McpSettingsPage({ onDirtyChange }: McpSettingsPageProps) {
  const { t } = useFrontendConfig()
  const { showToast } = useToast()
  const management = useMcpManagement()
  const [view, setView] = useState<McpSettingsView>('list')
  const [selectedServerId, setSelectedServerId] = useState<string | null>(null)
  const [editingServerSnapshot, setEditingServerSnapshot] = useState<McpServerDetailsView | null>(
    null
  )
  const [editorDirty, setEditorDirty] = useState(false)
  const [pendingDelete, setPendingDelete] = useState<McpServerListItem | null>(null)
  const [pendingEnableConfirmation, setPendingEnableConfirmation] =
    useState<McpServerListItem | null>(null)
  const pageTitleRef = useRef<HTMLHeadingElement | null>(null)
  const enablePreflightRef = useRef<Set<string>>(new Set())
  const loadDetails = management.loadDetails

  const selectedListItem = useMemo(
    () =>
      management.state.output?.servers.find((server) => server.serverId === selectedServerId) ??
      null,
    [management.state.output, selectedServerId]
  )
  const selectedDetails = selectedServerId
    ? (management.detailsById.get(selectedServerId) ?? null)
    : null
  const currentSelectedDetails =
    selectedDetails &&
    selectedListItem &&
    selectedDetails.registryRevision === selectedListItem.registryRevision &&
    selectedDetails.configEpoch === selectedListItem.configEpoch &&
    selectedDetails.configDigest === selectedListItem.configDigest
      ? selectedDetails
      : null
  useEffect(() => {
    onDirtyChange?.(editorDirty)
  }, [editorDirty, onDirtyChange])
  useEffect(() => () => onDirtyChange?.(false), [onDirtyChange])

  useEffect(() => {
    if (
      view === 'edit' &&
      selectedListItem &&
      (!selectedDetails ||
        selectedDetails.registryRevision !== selectedListItem.registryRevision ||
        selectedDetails.configEpoch !== selectedListItem.configEpoch ||
        selectedDetails.configDigest !== selectedListItem.configDigest)
    ) {
      void loadDetails(selectedListItem).catch((error: unknown) => {
        showSafeError(error, showToast, t)
        setEditingServerSnapshot(null)
        setSelectedServerId(null)
        setView('list')
      })
    }
  }, [loadDetails, selectedDetails, selectedListItem, showToast, t, view])

  useEffect(() => {
    if (view === 'edit' && !editingServerSnapshot && currentSelectedDetails) {
      setEditingServerSnapshot(currentSelectedDetails)
    }
  }, [currentSelectedDetails, editingServerSnapshot, view])

  useEffect(() => {
    if (
      management.state.status === 'ready' &&
      selectedServerId &&
      !selectedListItem &&
      view !== 'add'
    ) {
      if (view === 'edit' && editorDirty) return
      setSelectedServerId(null)
      setEditingServerSnapshot(null)
      setEditorDirty(false)
      setView('list')
    }
  }, [editorDirty, management.state.status, selectedListItem, selectedServerId, view])

  const editServer = (server: McpServerListItem) => {
    setSelectedServerId(server.serverId)
    const cached = management.detailsById.get(server.serverId)
    setEditingServerSnapshot(
      cached &&
        cached.registryRevision === server.registryRevision &&
        cached.configEpoch === server.configEpoch &&
        cached.configDigest === server.configDigest
        ? cached
        : null
    )
    setView('edit')
  }

  const runServerOperation = useCallback(
    async (operation: () => Promise<unknown>) => {
      try {
        await operation()
      } catch (error) {
        showSafeError(error, showToast, t)
      }
    },
    [showToast, t]
  )

  const requestServerEnabledChange = async (server: McpServerListItem, enabled: boolean) => {
    if (!enabled) {
      await runServerOperation(() => management.setServerEnabled(server, false))
      return
    }
    if (server.launchAuthorizationState !== 'authorized') {
      setPendingEnableConfirmation(server)
      return
    }
    if (enablePreflightRef.current.has(server.serverId)) return
    enablePreflightRef.current.add(server.serverId)

    try {
      // List rows deliberately use a cheap structural authorization check.
      // Resolve one live detail immediately before enable so a changed local
      // executable or script returns to the existing user confirmation flow.
      const current = await management.loadDetails(server)
      if (!current) return
      if (current.launchAuthorizationState !== 'authorized') {
        setPendingEnableConfirmation(current)
        return
      }
      await management.setServerEnabled(current, true)
    } catch (error) {
      const details = getMcpManagementErrorDetails(error)
      if (mcpOperationNeedsLaunchAuthorization(details)) {
        try {
          const current = await management.loadDetails(server)
          if (current) setPendingEnableConfirmation(current)
        } catch (refreshError) {
          showSafeError(refreshError, showToast, t)
        }
        return
      }
      showSafeError(error, showToast, t)
    } finally {
      enablePreflightRef.current.delete(server.serverId)
    }
  }

  const confirmEnable = async () => {
    const server = pendingEnableConfirmation
    if (!server) return
    try {
      await management.setServerEnabled(server, true)
      setPendingEnableConfirmation(null)
    } catch (error) {
      const details = getMcpManagementErrorDetails(error)
      if (mcpOperationNeedsLaunchAuthorization(details)) {
        try {
          const current = await management.loadDetails(server)
          if (current) {
            setPendingEnableConfirmation(current)
            return
          }
        } catch (refreshError) {
          showSafeError(refreshError, showToast, t)
          setPendingEnableConfirmation(null)
          return
        }
      }
      showSafeError(error, showToast, t)
      setPendingEnableConfirmation(null)
    }
  }

  const submitAdd = async (draft: McpServerDraft) => {
    try {
      const server = await management.addServer(draft)
      if (!server) return
      setSelectedServerId(null)
      setEditorDirty(false)
      setView('list')
      showToast(t('mcp.toast.savedDisabled'), { durationMs: 3200 })
    } catch (error) {
      showSafeError(error, showToast, t)
    }
  }

  const submitUpdate = async (draft: McpServerDraft) => {
    if (!editingServerSnapshot) return
    if (!selectedListItem) {
      showToast(t('mcp.toast.stateChanged'), { durationMs: 3200 })
      void management.refresh()
      return
    }
    if (
      editingServerSnapshot.configEpoch !== selectedListItem.configEpoch ||
      editingServerSnapshot.configDigest !== selectedListItem.configDigest
    ) {
      showToast(t('mcp.toast.stateChanged'), { durationMs: 3200 })
      void management.refresh()
      return
    }
    try {
      const server = await management.updateServer(selectedListItem, draft)
      if (!server) return
      setEditorDirty(false)
      setEditingServerSnapshot(null)
      setSelectedServerId(null)
      setView('list')
      showToast(t('mcp.toast.saved'), { durationMs: 3200 })
    } catch (error) {
      showSafeError(error, showToast, t)
    }
  }

  const confirmDelete = async () => {
    const server = pendingDelete
    if (!server) return
    try {
      await management.deleteServer(server)
      setPendingDelete(null)
      if (selectedServerId === server.serverId) {
        setEditorDirty(false)
        setEditingServerSnapshot(null)
        setSelectedServerId(null)
        setView('list')
      }
    } catch (error) {
      const details = getMcpManagementErrorDetails(error)
      if (mcpOperationNeedsAuthoritativeConfirmation(details)) {
        setPendingDelete(null)
        showToast(t('mcp.toast.deleteNeedsConfirmation'), { durationMs: 4200 })
      } else {
        showToast(
          details.message ? toSafeMcpDisplayText(details.message) : t('mcp.error.unknown'),
          { durationMs: 3200 }
        )
      }
    }
  }

  const output = management.state.output

  return (
    <article className="settings-list-page mcp-settings-page">
      <header className="mcp-settings-header">
        <div>
          <h1 ref={pageTitleRef} tabIndex={-1}>
            {t('settings.page.mcp')}
          </h1>
          <p className="settings-list-page__description">{t('mcp.page.description')}</p>
        </div>
        {view === 'list' && (
          <div className="mcp-settings-header__actions">
            <McpServerListRefreshButton
              disabled={management.state.isRefreshing}
              onRefresh={() => void management.refresh()}
            />
            <button
              className="mcp-secondary-button"
              onClick={() => {
                setSelectedServerId(null)
                setView('add')
              }}
              type="button"
            >
              <Plus aria-hidden="true" />
              {t('mcp.actions.addServer')}
            </button>
          </div>
        )}
      </header>

      {view === 'list' && (
        <>
          {management.state.status === 'loading' && (
            <div className="mcp-page-state" role="status">
              <LoaderCircle aria-hidden="true" className="mcp-spinner" />
              <span>{t('mcp.loading')}</span>
            </div>
          )}

          {management.state.status === 'error' && (
            <div className="mcp-page-state mcp-page-state--error" role="alert">
              <AlertTriangle aria-hidden="true" />
              <div>
                <strong>{t('mcp.loadFailed')}</strong>
                <p>
                  {management.state.errorMessage
                    ? toSafeMcpDisplayText(management.state.errorMessage)
                    : t('mcp.error.unknown')}
                </p>
              </div>
              <button
                className="mcp-secondary-button"
                onClick={() => void management.refresh()}
                type="button"
              >
                <RefreshCw aria-hidden="true" />
                {t('mcp.actions.retry')}
              </button>
            </div>
          )}

          {output && management.state.errorMessage !== null && (
            <div className="mcp-safe-error" role="status">
              <AlertTriangle aria-hidden="true" />
              <p>
                {management.state.errorMessage
                  ? toSafeMcpDisplayText(management.state.errorMessage)
                  : t('mcp.error.unknown')}
              </p>
            </div>
          )}

          {output &&
            (output.servers.length === 0 ? (
              <div className="mcp-empty-state">
                <Server aria-hidden="true" />
                <strong>{t('mcp.empty.title')}</strong>
                <p>{t('mcp.empty.description')}</p>
              </div>
            ) : (
              <McpServerList
                onEdit={editServer}
                onSetEnabled={requestServerEnabledChange}
                pendingOperations={management.pendingOperations}
                servers={output.servers}
              />
            ))}
        </>
      )}

      {view === 'add' && (
        <>
          <h2 className="mcp-subpage-title">{t('mcp.add.title')}</h2>
          <McpServerEditor
            busy={management.isAdding}
            onCancel={() => {
              setEditorDirty(false)
              setView('list')
            }}
            onDirtyChange={setEditorDirty}
            onSelectExecutable={management.selectExecutable}
            onSelectWorkingDirectory={management.selectWorkingDirectory}
            onSubmit={submitAdd}
          />
        </>
      )}

      {view === 'edit' && editingServerSnapshot && (
        <>
          <h2 className="mcp-subpage-title">{t('mcp.edit.title')}</h2>
          <McpServerEditor
            busy={Boolean(management.pendingOperations.get(editingServerSnapshot.serverId))}
            initial={editingServerSnapshot}
            onCancel={() => {
              setEditorDirty(false)
              setEditingServerSnapshot(null)
              setSelectedServerId(null)
              setView('list')
            }}
            onDelete={() => setPendingDelete(selectedListItem)}
            onDirtyChange={setEditorDirty}
            onSelectExecutable={management.selectExecutable}
            onSelectWorkingDirectory={management.selectWorkingDirectory}
            onSubmit={submitUpdate}
          />
        </>
      )}

      {view === 'edit' && selectedListItem && !editingServerSnapshot && (
        <div className="mcp-page-state" role="status">
          <LoaderCircle aria-hidden="true" className="mcp-spinner" />
          {t('mcp.detail.loading')}
        </div>
      )}

      {pendingDelete && (
        <ConfirmationDialog
          cancelLabel={t('mcp.actions.cancel')}
          confirmLabel={t('mcp.actions.confirmDelete')}
          description={t('mcp.delete.description')}
          fallbackFocusRef={pageTitleRef}
          onCancel={() => setPendingDelete(null)}
          onConfirm={confirmDelete}
          title={`${t('mcp.delete.title')}: ${toSafeMcpDisplayText(pendingDelete.displayName, 256)}`}
        />
      )}

      {pendingEnableConfirmation && (
        <ConfirmationDialog
          cancelLabel={t('mcp.actions.cancel')}
          confirmLabel={t('mcp.actions.enable')}
          confirmVariant="primary"
          description={t('mcp.authorization.confirmDescription')}
          fallbackFocusRef={pageTitleRef}
          onCancel={() => setPendingEnableConfirmation(null)}
          onConfirm={confirmEnable}
          title={t('mcp.authorization.confirmTitle').replace(
            '{serverName}',
            toSafeMcpDisplayText(pendingEnableConfirmation.displayName, 256)
          )}
        />
      )}
    </article>
  )
}

type Translate = ReturnType<typeof useFrontendConfig>['t']

function showSafeError(
  error: unknown,
  showToast: (message: string, options?: { durationMs?: number }) => void,
  t: Translate
): void {
  const details = getMcpManagementErrorDetails(error)
  showToast(
    details.code === 'conflict'
      ? t('mcp.toast.stateChanged')
      : details.message
        ? toSafeMcpDisplayText(details.message)
        : t('mcp.error.unknown'),
    { durationMs: 3200 }
  )
}
