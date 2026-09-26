import type { WorkflowInstance, WorkflowRecord, WorkflowResponse } from '@mycopilot/protocol'
import { ArrowLeft, List, Network, Settings2, RefreshCw, Trash2 } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from 'react'
import { ConfirmationDialog } from '../../../components/dialog/ConfirmationDialog'
import { Tooltip } from '../../../components/overlay/Tooltip'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { useModelSettings } from '../../../config/ModelSettingsProvider'
import type { AppProject } from '../../../config/projectConfig'
import type { ChatComposerDraft, ChatConversation } from '../../chat/chatTypes'
import type { ConversationAttentionById } from '../../chat/useConversationAttention'
import { SettingsSelect } from '../../settings/components/SettingsSelect'
import { requestWorkflows } from '../workflowClient'
import { workflowErrorDetail } from '../workflowErrors'
import { WorkflowBindingCanvas } from './WorkflowBindingCanvas'
import { WorkflowColorPicker } from './WorkflowColorPicker'
import { WorkflowMonitorPage } from './WorkflowMonitorPage'
import { useWorkflowActivity } from './useWorkflowActivity'
import { projectWorkflowText, pickUnusedWorkflowColor } from './projectWorkflowText'
import '../workflows.css'
import './projectWorkflows.css'

export interface WorkflowsPageProps {
  conversations: readonly ChatConversation[]
  conversationAttention?: ConversationAttentionById
  conversationDrafts?: Readonly<Record<string, ChatComposerDraft>>
  projects: readonly AppProject[]
  onManageTemplates: () => void
  onClose?: () => void
  onCommitted: (response: WorkflowResponse) => void | Promise<void>
  onBeforeCommit?: (conversationIds: readonly string[]) => Promise<void>
  onDirtyChange?: (dirty: boolean) => void
  onBusyChange?: (busy: boolean) => void
  initialMonitorId?: string | null
  onMonitorChange?: (instanceId: string | null) => void
  onOpenConversation?: (conversationId: string) => void
}

interface BindingDraft {
  id: string
  name: string
  color: string
  record: WorkflowRecord | null
  instance: WorkflowInstance | null
  bindings: Record<string, string | null>
  baseline: string
}

const draftKey = (draft: Pick<BindingDraft, 'name' | 'color' | 'bindings' | 'record'>) =>
  JSON.stringify([
    draft.record?.definition.id ?? null,
    draft.name,
    draft.color,
    Object.entries(draft.bindings).sort(([a], [b]) => a.localeCompare(b))
  ])

// Keep existing cards in place when the host orders a changed instance by updatedAt.
function keepInstanceOrder(current: WorkflowInstance[], incoming: WorkflowInstance[]) {
  const nextById = new Map(incoming.map((instance) => [instance.id, instance]))
  const currentIds = new Set(current.map((instance) => instance.id))
  return [
    ...incoming.filter((instance) => !currentIds.has(instance.id)),
    ...current.flatMap((instance) => {
      const next = nextById.get(instance.id)
      return next ? [next] : []
    })
  ]
}

export function WorkflowsPage({
  conversations,
  conversationAttention,
  conversationDrafts,
  onManageTemplates,
  onClose,
  onCommitted,
  onBeforeCommit,
  onDirtyChange,
  onBusyChange,
  initialMonitorId,
  onMonitorChange,
  onOpenConversation
}: WorkflowsPageProps) {
  const { language } = useFrontendConfig()
  const { models } = useModelSettings()
  const t = useMemo(() => projectWorkflowText(language), [language])
  const [records, setRecords] = useState<WorkflowRecord[]>([])
  const [instances, setInstances] = useState<WorkflowInstance[]>([])
  const [monitorId, setMonitorId] = useState<string | null>(initialMonitorId ?? null)
  useEffect(() => {
    setMonitorId(initialMonitorId ?? null)
  }, [initialMonitorId])
  const openMonitor = (id: string | null) => {
    setMonitorId(id)
    onMonitorChange?.(id)
  }
  const runningInstanceIds = useWorkflowActivity(monitorId ? [] : instances, conversations)
  const [loading, setLoading] = useState(true)
  const [hasSnapshot, setHasSnapshot] = useState(false)
  const [error, setErrorMessage] = useState('')
  const [errorDetail, setErrorDetail] = useState('')
  const setError = useCallback((message: string, cause?: unknown) => {
    setErrorMessage(message)
    setErrorDetail(workflowErrorDetail(cause))
  }, [])
  const [busy, setBusy] = useState(false)
  const busyRef = useRef(false)
  // Transient switch locks use aria-disabled plus handler guards to preserve appearance and focus.
  const [togglingId, setTogglingId] = useState<string | null>(null)
  const togglingIdRef = useRef<string | null>(null)
  const [draft, setDraft] = useState<BindingDraft | null>(null)
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null)
  const [pendingDelete, setPendingDelete] = useState<WorkflowInstance | null>(null)
  const [confirmDiscard, setConfirmDiscard] = useState(false)
  const [pendingTemplate, setPendingTemplate] = useState<WorkflowRecord | null>(null)
  const [versionNotice, setVersionNotice] = useState('')
  const [bindingNotice, setBindingNotice] = useState('')
  const [pendingSync, setPendingSync] = useState<WorkflowResponse | null>(null)
  const requestGeneration = useRef(0)
  const dirty = !!draft && draftKey(draft) !== draft.baseline

  useEffect(() => {
    onDirtyChange?.(dirty)
  }, [dirty, onDirtyChange])
  useEffect(() => {
    onBusyChange?.(busy || togglingId !== null)
  }, [busy, togglingId, onBusyChange])
  useEffect(
    () => () => {
      onDirtyChange?.(false)
      onBusyChange?.(false)
    },
    [onDirtyChange, onBusyChange]
  )

  const reload = useCallback(async () => {
    const generation = ++requestGeneration.current
    setLoading(true)
    setError('')
    try {
      const [templates, workflows] = await Promise.all([
        requestWorkflows({ operation: 'list' }),
        requestWorkflows({ operation: 'listInstances' })
      ])
      if (generation !== requestGeneration.current) return
      setRecords(templates.records)
      setInstances((current) => keepInstanceOrder(current, workflows.instances ?? []))
      setHasSnapshot(true)
    } catch (cause) {
      if (generation === requestGeneration.current)
        setError(
          language === 'zh-CN'
            ? '工作流加载失败，请重试。'
            : 'Could not load workflows. Please retry.',
          cause
        )
    } finally {
      if (generation === requestGeneration.current) setLoading(false)
    }
  }, [language, setError, setLoading, setRecords, setInstances, setHasSnapshot])

  useEffect(() => {
    void reload()
    return () => {
      requestGeneration.current += 1
    }
  }, [reload])

  useEffect(() => {
    const changed = () => {
      if (busyRef.current || togglingIdRef.current || pendingSync) return
      if (draft) {
        setVersionNotice(
          t(
            '工作流信息有更新。刷新版本后可保留当前绑定并重新确认。',
            'Workflow information changed. Refresh to keep your staged bindings and review the latest version.'
          )
        )
      } else void reload()
    }
    window.addEventListener('captain:workflows-changed', changed)
    return () => window.removeEventListener('captain:workflows-changed', changed)
  }, [draft, pendingSync, reload, t])

  const agents = draft?.record?.definition.nodes.filter((node) => node.kind === 'agent') ?? []
  const readyRecords = records.filter((record) => record.enabled && record.issues.length === 0)
  const selectedNode = agents.find((node) => node.id === selectedNodeId)
  const savedBindings = useMemo(
    () =>
      Object.fromEntries(
        draft?.instance?.bindings.map((binding) => [binding.nodeId, binding.conversationId]) ?? []
      ),
    [draft?.instance]
  )
  const activeConversations = useMemo(
    () => conversations.filter((item) => !item.archivedAt && !item.pendingArchivedAt),
    [conversations]
  )
  const readOnly = !!draft?.instance?.running
  const unavailableColors = instances
    .filter((instance) => instance.enabled && instance.id !== draft?.id)
    .map((instance) => instance.color)
  const colorOccupied =
    !!draft && unavailableColors.some((color) => color.toLowerCase() === draft.color.toLowerCase())
  const noColorsMessage = t(
    '可选颜色已全部被占用，请先停用或调整其他工作流。',
    'All available colors are in use. Disable or adjust another workflow first.'
  )
  const colorOccupiedMessage = t(
    '这个颜色已被其他工作流使用，请选择其他颜色。',
    'This color is used by another workflow. Choose a different color.'
  )

  const beginBinding = (
    record: WorkflowRecord | null,
    instance: WorkflowInstance | null = null
  ) => {
    if (pendingSync || busyRef.current || (instance && togglingIdRef.current === instance.id))
      return
    const existing = new Map(
      instance?.bindings.map((binding) => [binding.nodeId, binding.conversationId])
    )
    const bindings = Object.fromEntries(
      (record?.definition.nodes ?? [])
        .filter((node) => node.kind === 'agent')
        .map((node) => [node.id, existing.get(node.id) ?? null])
    )
    const color =
      instance?.color ??
      pickUnusedWorkflowColor(
        instances.filter((workflow) => workflow.enabled).map((workflow) => workflow.color)
      )
    if (!color) {
      setError(noColorsMessage)
      return
    }
    const next = {
      id: instance?.id ?? crypto.randomUUID(),
      name: instance?.name ?? record?.definition.name ?? '',
      color,
      record,
      instance,
      bindings,
      baseline: ''
    }
    next.baseline = draftKey(next)
    setDraft(next)
    setSelectedNodeId(null)
    setError('')
    setVersionNotice('')
  }

  const selectTemplate = (record: WorkflowRecord) => {
    if (!draft || draft.instance || busyRef.current) return
    const next = {
      ...draft,
      record,
      name: record.definition.name,
      bindings: Object.fromEntries(
        record.definition.nodes
          .filter((node) => node.kind === 'agent')
          .map((node) => [node.id, null])
      )
    }
    next.baseline = draftKey(next)
    setDraft(next)
    setSelectedNodeId(null)
    setError('')
    setVersionNotice('')
  }

  const requestTemplate = (templateId: string) => {
    const record = readyRecords.find((item) => item.definition.id === templateId)
    if (!record || draft?.record?.definition.id === templateId) return
    if (dirty) {
      setPendingTemplate(record)
      setConfirmDiscard(true)
    } else selectTemplate(record)
  }

  const refreshVersion = async () => {
    if (!draft || busyRef.current || togglingIdRef.current) return
    busyRef.current = true
    setBusy(true)
    try {
      const [templates, workflows] = await Promise.all([
        requestWorkflows({ operation: 'list' }),
        requestWorkflows({ operation: 'listInstances' })
      ])
      const record = templates.records.find(
        (item) => item.definition.id === draft.record?.definition.id
      )
      const instance = draft.instance
        ? workflows.instances?.find((item) => item.id === draft.id)
        : null
      setRecords(templates.records)
      setInstances((current) => keepInstanceOrder(current, workflows.instances ?? []))
      if (!draft.record) {
        setError('')
        setVersionNotice('')
        return
      }
      if (!record || (draft.instance && !instance)) {
        setError(
          t(
            '这个模板或工作流已被移除。请返回列表重新选择。',
            'This template or workflow was removed. Return to the list and choose another.'
          )
        )
        return
      }
      const prior = new Map(
        instance?.bindings.map((binding) => [binding.nodeId, binding.conversationId])
      )
      const bindings = Object.fromEntries(
        record.definition.nodes
          .filter((node) => node.kind === 'agent')
          .map((node) => [
            node.id,
            node.id in draft.bindings ? draft.bindings[node.id] : (prior.get(node.id) ?? null)
          ])
      )
      setDraft({ ...draft, record, instance: instance ?? null, bindings })
      setError('')
      setVersionNotice(
        t(
          '已加载最新版本并保留当前绑定，请检查后重新确认。',
          'Loaded the latest version and retained your bindings. Review them before confirming again.'
        )
      )
    } catch (cause) {
      setError(
        t(
          '刷新失败，当前绑定已保留，请重试。',
          'Could not refresh. Your staged bindings are preserved. Please retry.'
        ),
        cause
      )
    } finally {
      busyRef.current = false
      setBusy(false)
    }
  }

  const occupancy = (conversationId: string, nodeId: string): string | null => {
    const other = instances.find(
      (instance) =>
        instance.id !== draft?.id &&
        instance.bindings.some((binding) => binding.conversationId === conversationId)
    )
    if (other)
      return t(
        `这个对话已经加入其他工作流“${other.name}”，不能重复分配。`,
        `This conversation already belongs to another workflow, “${other.name}”, and cannot be assigned again.`
      )
    const sibling = agents.find(
      (node) => node.id !== nodeId && draft?.bindings[node.id] === conversationId
    )
    return sibling
      ? t(
          `这个对话已分配给当前工作流的“${sibling.name}”，不能重复分配。`,
          `This conversation is already assigned to “${sibling.name}” in this workflow and cannot be assigned again.`
        )
      : null
  }

  const bind = (nodeId: string, conversationId: string | null) => {
    if (!draft || busyRef.current || readOnly) return
    if (conversationId && !activeConversations.some((item) => item.id === conversationId)) {
      setError(
        t(
          '这个对话当前不可用，请选择其他对话。',
          'This conversation is unavailable. Choose another conversation.'
        )
      )
      return
    }
    const occupied = conversationId ? occupancy(conversationId, nodeId) : null
    if (occupied) {
      setError('')
      setBindingNotice(occupied)
      return
    }
    setError('')
    setSelectedNodeId(nodeId)
    setDraft({ ...draft, bindings: { ...draft.bindings, [nodeId]: conversationId } })
  }

  const acceptCommit = async (response: WorkflowResponse) => {
    requestGeneration.current += 1
    setLoading(false)
    setInstances((current) => keepInstanceOrder(current, response.instances ?? []))
    setRecords(response.records)
    setDraft(null)
    setSelectedNodeId(null)
    setPendingDelete(null)
    setError('')
    onDirtyChange?.(false)
    await synchronizeCommit(response)
  }

  const synchronizeCommit = async (response: WorkflowResponse) => {
    try {
      await onCommitted(response)
      setPendingSync(null)
    } catch (cause) {
      setPendingSync(response)
      setError(
        t(
          '工作流已保存，对话列表刷新失败，请重试同步。',
          'The workflow was saved, but the conversation list could not refresh. Retry synchronization.'
        ),
        cause
      )
    }
  }

  const retrySync = async () => {
    if (!pendingSync || busyRef.current) return
    busyRef.current = true
    setBusy(true)
    try {
      await onCommitted(pendingSync)
      setPendingSync(null)
      setError('')
    } catch (cause) {
      setError(
        t(
          '同步失败，已保存的工作流不会重复创建，请重试。',
          'Synchronization failed. The saved workflow will not be created again. Please retry.'
        ),
        cause
      )
    } finally {
      busyRef.current = false
      setBusy(false)
    }
  }

  const commit = async () => {
    if (
      !draft?.record ||
      busyRef.current ||
      togglingIdRef.current ||
      pendingSync ||
      readOnly ||
      !draft.name.trim()
    )
      return
    if (colorOccupied) {
      setError(colorOccupiedMessage)
      return
    }
    busyRef.current = true
    setBusy(true)
    setError('')
    try {
      await onBeforeCommit?.(Object.values(draft.bindings).filter((id): id is string => !!id))
      const response = await requestWorkflows({
        operation: 'saveInstance',
        id: draft.id,
        templateId: draft.record.definition.id,
        name: draft.name.trim(),
        color: draft.color,
        bindings: agents.map((node) => ({
          nodeId: node.id,
          conversationId: draft.bindings[node.id] ?? null
        })),
        expectedRevision: draft.instance?.revision ?? 0,
        expectedTemplateRevision: draft.record.revision
      })
      await acceptCommit(response)
    } catch (cause) {
      if (workflowErrorDetail(cause).includes('workflow_conversation_already_bound')) {
        setError('')
        setBindingNotice(
          t(
            '这个对话已经加入其他工作流，不能重复分配。',
            'This conversation already belongs to another workflow and cannot be assigned again.'
          )
        )
        try {
          const current = await requestWorkflows({ operation: 'listInstances' })
          setInstances(current.instances ?? [])
        } catch {
          /* Preserve the staged configuration when the catalog cannot refresh. */
        }
        return
      }
      if (workflowErrorDetail(cause).includes('workflow_color_in_use')) {
        setError(colorOccupiedMessage)
        try {
          const current = await requestWorkflows({ operation: 'listInstances' })
          setInstances(current.instances ?? [])
        } catch {
          /* Keep the staged configuration available for retry. */
        }
        return
      }
      setError(
        t(
          '保存失败。模板、对话占用或工作流状态可能已改变，请保留当前配置并重新检查。',
          'Could not save. The template, conversation assignments, or workflow state may have changed. Review your bindings and try again.'
        ),
        cause
      )
    } finally {
      busyRef.current = false
      setBusy(false)
    }
  }

  const remove = async () => {
    if (!pendingDelete || busyRef.current || togglingIdRef.current) return
    busyRef.current = true
    setBusy(true)
    try {
      const response = await requestWorkflows({
        operation: 'deleteInstance',
        id: pendingDelete.id,
        expectedRevision: pendingDelete.revision
      })
      await acceptCommit(response)
    } catch (cause) {
      setError(
        t(
          '移除失败，工作流状态可能已改变。请刷新后重试。',
          'Could not remove this workflow. Refresh its state and try again.'
        ),
        cause
      )
      setPendingDelete(null)
    } finally {
      busyRef.current = false
      setBusy(false)
    }
  }

  const setInstanceEnabled = async (instance: WorkflowInstance) => {
    if (busyRef.current || togglingIdRef.current || pendingSync) return
    // A background read started before this mutation must not overwrite its response.
    requestGeneration.current += 1
    setLoading(false)
    togglingIdRef.current = instance.id
    setTogglingId(instance.id)
    setError('')
    try {
      const response = await requestWorkflows({
        operation: 'setInstanceEnabled',
        id: instance.id,
        enabled: !instance.enabled,
        expectedRevision: instance.revision
      })
      setInstances((current) => keepInstanceOrder(current, response.instances ?? []))
      setRecords(response.records)
      await synchronizeCommit(response)
    } catch (cause) {
      setError(
        workflowErrorDetail(cause).includes('workflow_color_in_use')
          ? colorOccupiedMessage
          : t(
              '切换失败，请刷新后检查工作流配置。',
              'Could not change workflow state. Refresh and check its configuration.'
            ),
        cause
      )
    } finally {
      togglingIdRef.current = null
      setTogglingId(null)
    }
  }

  const back = () => {
    if (busyRef.current) return
    if (draft) {
      if (dirty) {
        setPendingTemplate(null)
        setConfirmDiscard(true)
        return
      }
      setDraft(null)
      setSelectedNodeId(null)
      setError('')
    } else onClose?.()
  }

  if (monitorId) {
    const instance = instances.find((item) => item.id === monitorId)
    const record = records.find((item) => item.definition.id === instance?.templateId)
    if (instance && record) {
      return (
        <WorkflowMonitorPage
          key={instance.id}
          instance={instance}
          graph={record.definition}
          conversations={conversations}
          conversationAttention={conversationAttention}
          conversationDrafts={conversationDrafts}
          models={models}
          onBack={() => openMonitor(null)}
          onOpenConversation={(id) => onOpenConversation?.(id)}
        />
      )
    }
    return (
      <section className="project-workflows" aria-label={t('工作流流程图', 'Workflow diagram')}>
        <header className="project-workflows__header">
          <div className="project-workflows__identity">
            <Tooltip content={t('返回工作流', 'Back to workflows')}>
              <button
                type="button"
                className="workflow-icon-button"
                aria-label={t('返回工作流', 'Back to workflows')}
                onClick={() => openMonitor(null)}
              >
                <ArrowLeft aria-hidden="true" />
              </button>
            </Tooltip>
            <h1>{t('工作流流程图', 'Workflow diagram')}</h1>
          </div>
        </header>
        <div className="project-workflows__empty" role={error ? 'alert' : 'status'}>
          <p>
            {loading
              ? t('正在加载工作流…', 'Loading workflows…')
              : error || t('工作流或模板已不可用', 'This workflow or its template is unavailable')}
          </p>
          {!loading && (
            <button className="workflow-button" type="button" onClick={() => void reload()}>
              <RefreshCw aria-hidden="true" />
              {t('重试', 'Retry')}
            </button>
          )}
        </div>
      </section>
    )
  }

  return (
    <section
      className={`project-workflows${draft ? ' project-workflows--editing' : ''}`}
      aria-label={t('工作流管理', 'Workflow management')}
    >
      {!draft && (
        <header className="project-workflows__header">
          <div className="project-workflows__identity">
            {onClose && (
              <Tooltip content={t('返回', 'Back')}>
                <button
                  type="button"
                  className="workflow-icon-button"
                  aria-label={t('返回', 'Back')}
                  disabled={busy}
                  onClick={back}
                >
                  <ArrowLeft aria-hidden="true" />
                </button>
              </Tooltip>
            )}
            <h1>{t('工作流', 'Workflows')}</h1>
          </div>
          <div className="project-workflows__actions">
            <button
              type="button"
              className="workflow-button"
              disabled={busy || !!pendingSync}
              onClick={onManageTemplates}
            >
              <List aria-hidden="true" />
              {t('管理工作流模板', 'Manage workflow templates')}
            </button>
            <button
              type="button"
              className="workflow-button workflow-button--primary"
              disabled={loading || !hasSnapshot || busy || !!pendingSync}
              onClick={() => beginBinding(null)}
            >
              <Network aria-hidden="true" />
              {t('激活新工作流', 'Activate new workflow')}
            </button>
          </div>
        </header>
      )}
      {error && (
        <div className="project-workflows__notice is-error" role="alert">
          <div className="project-workflows__error-copy">
            <span>{error}</span>
            {errorDetail && (
              <details className="project-workflows__error-detail">
                <summary>{t('错误详情', 'Error details')}</summary>
                <p>{errorDetail}</p>
              </details>
            )}
          </div>
          {pendingSync ? (
            <button
              className="workflow-button"
              type="button"
              disabled={busy}
              onClick={() => void retrySync()}
            >
              <RefreshCw />
              {t('重试同步', 'Retry synchronization')}
            </button>
          ) : draft ? (
            <button
              className="workflow-button"
              type="button"
              disabled={busy || !!togglingId}
              onClick={() => void refreshVersion()}
            >
              <RefreshCw />
              {t('刷新版本', 'Refresh version')}
            </button>
          ) : (
            <button
              className="workflow-icon-button"
              type="button"
              aria-label={t('重试', 'Retry')}
              disabled={busy || loading}
              onClick={() => void reload()}
            >
              <RefreshCw />
            </button>
          )}
        </div>
      )}
      {draft && versionNotice && (
        <div className="project-workflows__notice" role="status">
          <span>{versionNotice}</span>
          <button
            className="workflow-icon-button"
            type="button"
            aria-label={t('刷新版本', 'Refresh version')}
            disabled={busy || !!togglingId}
            onClick={() => void refreshVersion()}
          >
            <RefreshCw />
          </button>
        </div>
      )}
      {loading && !hasSnapshot ? (
        <div className="project-workflows__empty" role="status">
          {t('正在加载工作流…', 'Loading workflows…')}
        </div>
      ) : !hasSnapshot ? (
        <div className="project-workflows__empty">
          <Network />
          <p>{t('暂时无法读取工作流', 'Workflows could not be loaded')}</p>
        </div>
      ) : draft ? (
        <>
          <div
            className="project-workflows__binding-toolbar"
            role="toolbar"
            aria-label={t('工作流配置', 'Workflow configuration')}
          >
            <div className="project-workflows__template-field">
              <span>{t('工作流模板', 'Workflow template')}</span>
              <SettingsSelect
                ariaLabel={t('工作流模板', 'Workflow template')}
                className="project-workflows__template-select"
                value={draft.record?.definition.id ?? ''}
                disabled={busy || !!draft.instance}
                options={
                  draft.instance && draft.record
                    ? [{ value: draft.record.definition.id, label: draft.record.definition.name }]
                    : [
                        {
                          value: '',
                          label: t('请选择工作流模板', 'Choose a workflow template'),
                          disabled: true
                        },
                        ...readyRecords.map((record) => ({
                          value: record.definition.id,
                          label: record.definition.name
                        }))
                      ]
                }
                onChange={requestTemplate}
              />
            </div>
            <label>
              <span>{t('名称', 'Name')}</span>
              <input
                value={draft.name}
                maxLength={160}
                disabled={busy || readOnly}
                onChange={(event) => setDraft({ ...draft, name: event.target.value })}
              />
            </label>
            <div className="project-workflows__binding-actions">
              <WorkflowColorPicker
                color={draft.color}
                unavailableColors={unavailableColors}
                disabled={busy || readOnly}
                onChange={(color) => setDraft({ ...draft, color })}
              />
              {selectedNode && draft.bindings[selectedNode.id] && (
                <Tooltip content={t('解除对话分配', 'Remove conversation assignment')}>
                  <button
                    type="button"
                    className="workflow-icon-button"
                    aria-label={t('解除对话分配', 'Remove conversation assignment')}
                    disabled={busy || readOnly}
                    onClick={() => bind(selectedNode.id, null)}
                  >
                    <Trash2 aria-hidden="true" />
                  </button>
                </Tooltip>
              )}
            </div>
            <div className="project-workflows__commit-actions">
              <button type="button" className="workflow-button" disabled={busy} onClick={back}>
                {t('取消', 'Cancel')}
              </button>
              <button
                type="button"
                className="workflow-button workflow-button--primary"
                disabled={
                  busy ||
                  !!togglingId ||
                  !!pendingSync ||
                  readOnly ||
                  colorOccupied ||
                  !draft.record ||
                  !draft.name.trim() ||
                  (draft.record?.issues.length ?? 0) > 0
                }
                onClick={() => void commit()}
              >
                {t('激活', 'Activate')}
              </button>
            </div>
          </div>
          {draft.instance?.needsReview && (
            <div className="project-workflows__notice">
              {t(
                '模板已更新，请检查保留的绑定并补全新增节点。移除的节点不会删除真实对话。',
                'The template changed. Review retained bindings and fill new nodes. Removed nodes do not delete conversations.'
              )}
            </div>
          )}
          {readOnly && (
            <div className="project-workflows__notice">
              {t(
                '这个工作流正在运行，暂时不能修改对话绑定。',
                'This workflow is running. Conversation bindings cannot be changed yet.'
              )}
            </div>
          )}
          <div className="project-workflows__binding-content">
            {draft.record ? (
              <WorkflowBindingCanvas
                key={draft.record.definition.id}
                graph={draft.record.definition}
                color={draft.color}
                conversations={conversations}
                conversationDrafts={conversationDrafts}
                models={models}
                bindings={draft.bindings}
                savedBindings={savedBindings}
                selectedNodeId={selectedNodeId}
                disabled={busy || readOnly}
                onSelect={(id) => setSelectedNodeId((current) => (current === id ? null : id))}
                onBind={bind}
              />
            ) : (
              <div
                className="workflow-canvas-shell workflow-binding-canvas-shell project-workflows__blank-canvas"
                aria-label={t('空白工作流画布', 'Empty workflow canvas')}
              >
                <div className="project-workflows__empty">
                  <Network aria-hidden="true" />
                  <p>
                    {readyRecords.length
                      ? t('在上方选择工作流模板', 'Choose a workflow template above')
                      : t('还没有可用的工作流模板', 'No workflow templates are ready yet')}
                  </p>
                  {!readyRecords.length && (
                    <button type="button" className="workflow-button" onClick={onManageTemplates}>
                      <List aria-hidden="true" />
                      {t('管理工作流模板', 'Manage workflow templates')}
                    </button>
                  )}
                </div>
              </div>
            )}
          </div>
        </>
      ) : (
        <div className="project-workflows__library">
          {instances.map((instance) => {
            const record = records.find((item) => item.definition.id === instance.templateId)
            const complete =
              !!record &&
              record.definition.nodes
                .filter((node) => node.kind === 'agent')
                .every((node) =>
                  instance.bindings.some(
                    (binding) =>
                      binding.nodeId === node.id &&
                      activeConversations.some(
                        (conversation) => conversation.id === binding.conversationId
                      )
                  )
                )
            const conflicts = instances.some(
              (other) =>
                other.id !== instance.id &&
                other.enabled &&
                other.color.toLowerCase() === instance.color.toLowerCase()
            )
            const enableBlocked =
              !instance.enabled &&
              (!record ||
                record.issues.length > 0 ||
                instance.needsReview ||
                instance.templateRevision !== record.revision ||
                !complete ||
                conflicts)
            const switchTitle = instance.enabled
              ? t('停用工作流', 'Disable workflow')
              : conflicts
                ? colorOccupiedMessage
                : enableBlocked
                  ? t(
                      '请先检查模板并补全对话绑定',
                      'Review the template and complete conversation assignments first'
                    )
                  : t('启用工作流', 'Enable workflow')
            return (
              <article
                className={`project-workflows__instance${instance.enabled && runningInstanceIds.has(instance.id) ? ' is-running' : ''}`}
                key={instance.id}
                style={{ '--workflow-color': instance.color } as CSSProperties}
              >
                <svg className="project-workflows__activity" aria-hidden="true" focusable="false">
                  <rect className="project-workflows__activity-track" pathLength="100" />
                  <rect className="project-workflows__activity-trail" pathLength="100" />
                  <rect className="project-workflows__activity-head" pathLength="100" />
                </svg>
                <span
                  className="project-workflows__color-dot"
                  style={{ background: instance.color }}
                />
                <button
                  type="button"
                  className="project-workflows__instance-main"
                  aria-disabled={togglingId === instance.id || undefined}
                  disabled={busy || !record || !!pendingSync}
                  onClick={() => record && beginBinding(record, instance)}
                >
                  <strong>{instance.name}</strong>
                  <small>
                    {record?.definition.name ?? t('模板不可用', 'Template unavailable')} ·{' '}
                    {t(
                      `${instance.bindings.length} 个对话`,
                      `${instance.bindings.length} conversations`
                    )}
                  </small>
                </button>
                {(instance.running || instance.needsReview) && (
                  <span
                    className={`project-workflows__status${instance.needsReview ? ' needs-review' : ''}`}
                  >
                    {instance.running ? t('运行中', 'Running') : t('需要重新确认', 'Needs review')}
                  </span>
                )}
                <Tooltip content={switchTitle}>
                  <button
                    type="button"
                    role="switch"
                    className="project-workflows__switch"
                    aria-label={`${instance.enabled ? t('停用工作流', 'Disable workflow') : t('启用工作流', 'Enable workflow')} ${instance.name}`}
                    aria-checked={instance.enabled}
                    aria-busy={togglingId === instance.id || undefined}
                    aria-disabled={!!togglingId || undefined}
                    disabled={busy || !!pendingSync || enableBlocked}
                    onClick={() => void setInstanceEnabled(instance)}
                  >
                    <span aria-hidden="true" />
                  </button>
                </Tooltip>
                <Tooltip content={t('查看流程图', 'View workflow diagram')}>
                  <button
                    type="button"
                    className="workflow-icon-button project-workflows__diagram"
                    aria-label={`${t('查看流程图', 'View workflow diagram')} ${instance.name}`}
                    disabled={!record}
                    onClick={() => openMonitor(instance.id)}
                  >
                    <Network aria-hidden="true" />
                  </button>
                </Tooltip>
                <Tooltip content={t('配置', 'Configure')}>
                  <button
                    type="button"
                    className="workflow-icon-button project-workflows__configure"
                    aria-label={`${t('配置', 'Configure')} ${instance.name}`}
                    aria-disabled={togglingId === instance.id || undefined}
                    disabled={busy || !record || !!pendingSync}
                    onClick={() => record && beginBinding(record, instance)}
                  >
                    <Settings2 aria-hidden="true" />
                  </button>
                </Tooltip>
                <Tooltip content={t('移除工作流', 'Remove workflow')}>
                  <button
                    type="button"
                    className="workflow-icon-button"
                    aria-label={`${t('移除工作流', 'Remove workflow')} ${instance.name}`}
                    aria-disabled={!!togglingId || undefined}
                    disabled={busy || instance.running || !!pendingSync}
                    onClick={() => {
                      if (!togglingIdRef.current) setPendingDelete(instance)
                    }}
                  >
                    <Trash2 />
                  </button>
                </Tooltip>
              </article>
            )
          })}
          {!instances.length && (
            <div className="project-workflows__empty">
              <Network />
              <h2>{t('让对话一起协作', 'Bring conversations together')}</h2>
              <p>
                {t(
                  '从模板创建工作流，再为智能体绑定对话。',
                  'Create a workflow from a template, then bind conversations to its agents.'
                )}
              </p>
              <button
                type="button"
                className="workflow-button workflow-button--primary"
                disabled={busy || !!pendingSync}
                onClick={() => beginBinding(null)}
              >
                <Settings2 aria-hidden="true" />
                {t('激活新工作流', 'Activate new workflow')}
              </button>
            </div>
          )}
        </div>
      )}
      {bindingNotice && (
        <ConfirmationDialog
          title={t('无法分配对话', 'Cannot assign conversation')}
          description={bindingNotice}
          cancelLabel={t('关闭', 'Close')}
          confirmLabel={t('知道了', 'Got it')}
          confirmVariant="primary"
          showCancelButton={false}
          onCancel={() => setBindingNotice('')}
          onConfirm={() => setBindingNotice('')}
        />
      )}
      {pendingDelete && (
        <ConfirmationDialog
          title={t('移除工作流？', 'Remove workflow?')}
          description={t(
            `“${pendingDelete.name}”的绑定和颜色标记将被移除，所有对话都会保留。`,
            `Bindings and color markers for “${pendingDelete.name}” will be removed. All conversations will remain.`
          )}
          cancelLabel={t('取消', 'Cancel')}
          confirmLabel={t('移除', 'Remove')}
          onCancel={() => {
            if (!busyRef.current) setPendingDelete(null)
          }}
          onConfirm={remove}
        />
      )}
      {confirmDiscard && (
        <ConfirmationDialog
          title={t('放弃未保存的绑定？', 'Discard unsaved bindings?')}
          description={t(
            '这次选择不会应用到对话，已有工作流保持不变。',
            'These selections will not be applied to conversations. Existing workflows will remain unchanged.'
          )}
          cancelLabel={t('继续编辑', 'Keep editing')}
          confirmLabel={t('放弃修改', 'Discard changes')}
          onCancel={() => {
            setConfirmDiscard(false)
            setPendingTemplate(null)
          }}
          onConfirm={() => {
            setConfirmDiscard(false)
            if (pendingTemplate) selectTemplate(pendingTemplate)
            else setDraft(null)
            setPendingTemplate(null)
            setSelectedNodeId(null)
            setError('')
            setVersionNotice('')
          }}
        />
      )}
    </section>
  )
}
