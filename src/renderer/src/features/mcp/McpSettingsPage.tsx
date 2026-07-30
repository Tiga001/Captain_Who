import { AlertTriangle, LoaderCircle, Plus, RefreshCw, Server } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { McpServerDetailsView, McpServerListItem } from '@mycopilot/protocol'
import { ConfirmationDialog } from '../../components/dialog/ConfirmationDialog'
import { useToast } from '../../components/toast/ToastContext'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { McpServerDetail } from './McpServerDetail'
import { McpServerEditor } from './McpServerEditor'
import { McpServerList, McpServerListRefreshButton } from './McpServerList'
import { McpToolCatalog } from './McpToolCatalog'
import {
  getMcpManagementErrorDetails,
  mcpOperationNeedsAuthoritativeConfirmation
} from './mcpManagementErrors'
import type { McpServerDraft } from './mcpManagementInputs'
import { toSafeMcpDisplayText } from './mcpSafeDisplay'
import { useMcpManagement } from './useMcpManagement'
import './McpSettingsPage.css'

type McpSettingsView = 'list' | 'add' | 'detail' | 'edit'

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
  const pageTitleRef = useRef<HTMLHeadingElement | null>(null)
  const loadDetails = management.loadDetails
  const loadTools = management.loadTools

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
  const selectedCatalog = selectedServerId
    ? management.catalogsById.get(selectedServerId)
    : undefined

  useEffect(() => {
    onDirtyChange?.(editorDirty)
  }, [editorDirty, onDirtyChange])
  useEffect(() => () => onDirtyChange?.(false), [onDirtyChange])

  useEffect(() => {
    if (
      (view === 'detail' || view === 'edit') &&
      selectedListItem &&
      (!selectedDetails ||
        selectedDetails.registryRevision !== selectedListItem.registryRevision ||
        selectedDetails.configEpoch !== selectedListItem.configEpoch ||
        selectedDetails.configDigest !== selectedListItem.configDigest)
    ) {
      void loadDetails(selectedListItem).catch((error: unknown) => {
        showSafeError(error, showToast, t)
      })
    }
  }, [loadDetails, selectedDetails, selectedListItem, showToast, t, view])

  useEffect(() => {
    if (
      view === 'detail' &&
      selectedListItem &&
      (!selectedCatalog || selectedCatalog.status === 'idle')
    ) {
      void loadTools(selectedListItem).catch((error: unknown) => {
        showSafeError(error, showToast, t)
      })
    }
  }, [loadTools, selectedCatalog, selectedListItem, showToast, t, view])

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

  const openServer = (server: McpServerListItem) => {
    setEditingServerSnapshot(null)
    setSelectedServerId(server.serverId)
    setView('detail')
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

  const submitAdd = async (draft: McpServerDraft) => {
    try {
      const server = await management.addServer(draft)
      if (!server) return
      setSelectedServerId(server.serverId)
      setEditorDirty(false)
      setView('detail')
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
      setView('detail')
      showToast(
        server.launchAuthorizationState === 'authorized'
          ? t('mcp.toast.saved')
          : t('mcp.toast.savedNeedsAuthorization'),
        { durationMs: 3200 }
      )
    } catch (error) {
      showSafeError(error, showToast, t)
    }
  }

  const authorizeSelected = async () => {
    if (!selectedListItem) return
    try {
      const result = await management.authorizeLaunch(selectedListItem)
      if (result) showToast(t('mcp.toast.launchAuthorized'), { durationMs: 3200 })
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
              className="mcp-primary-button"
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
                onDelete={setPendingDelete}
                onOpen={openServer}
                onRestart={(server) =>
                  void runServerOperation(() => management.restartServer(server))
                }
                onSetEnabled={(server, enabled) =>
                  void runServerOperation(() =>
                    enabled ? management.enableServer(server) : management.disableServer(server)
                  )
                }
                pendingOperations={management.pendingOperations}
                servers={output.servers}
              />
            ))}
        </>
      )}

      {view === 'add' && (
        <>
          <h2 className="mcp-subpage-title">{t('mcp.add.title')}</h2>
          <p className="mcp-subpage-description">{t('mcp.add.description')}</p>
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
          <p className="mcp-subpage-description">{t('mcp.edit.description')}</p>
          <McpServerEditor
            busy={Boolean(management.pendingOperations.get(editingServerSnapshot.serverId))}
            initial={editingServerSnapshot}
            onCancel={() => {
              setEditorDirty(false)
              setEditingServerSnapshot(null)
              setView('detail')
            }}
            onDirtyChange={setEditorDirty}
            onSelectExecutable={management.selectExecutable}
            onSelectWorkingDirectory={management.selectWorkingDirectory}
            onSubmit={submitUpdate}
          />
        </>
      )}

      {(view === 'detail' || (view === 'edit' && !editingServerSnapshot)) &&
        selectedListItem &&
        !currentSelectedDetails && (
          <div className="mcp-page-state" role="status">
            <LoaderCircle aria-hidden="true" className="mcp-spinner" />
            {t('mcp.detail.loading')}
          </div>
        )}

      {view === 'detail' && selectedListItem && currentSelectedDetails && (
        <>
          <McpServerDetail
            onAuthorize={() => void authorizeSelected()}
            onBack={() => {
              setSelectedServerId(null)
              setView('list')
            }}
            onEdit={() => {
              setEditingServerSnapshot(currentSelectedDetails)
              setView('edit')
            }}
            onRestart={() =>
              void runServerOperation(() => management.restartServer(selectedListItem))
            }
            onSetEnabled={(enabled) =>
              void runServerOperation(() =>
                enabled
                  ? management.enableServer(selectedListItem)
                  : management.disableServer(selectedListItem)
              )
            }
            pending={management.pendingOperations.get(currentSelectedDetails.serverId)}
            server={currentSelectedDetails}
          />
          <McpToolCatalog
            catalog={
              selectedCatalog ?? {
                errorMessage: null,
                isRefreshing: false,
                status: 'idle',
                tools: []
              }
            }
            onLoad={() =>
              void management.loadTools(selectedListItem).catch((error: unknown) => {
                showSafeError(error, showToast, t)
              })
            }
            onLoadMore={() =>
              void management.loadTools(selectedListItem, true).catch((error: unknown) => {
                showSafeError(error, showToast, t)
              })
            }
            onRefresh={() =>
              void management.refreshCatalog(selectedListItem).catch((error: unknown) => {
                showSafeError(error, showToast, t)
              })
            }
            server={selectedListItem}
          />
        </>
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
