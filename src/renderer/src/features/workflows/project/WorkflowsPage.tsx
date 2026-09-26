import type { WorkflowInstance, WorkflowRecord, WorkflowResponse } from '@mycopilot/protocol'
import {
  ArrowLeft,
  Check,
  ChevronRight,
  GitBranch,
  Plus,
  RefreshCw,
  Search,
  Trash2,
  Unlink,
  X
} from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { ConfirmationDialog } from '../../../components/dialog/ConfirmationDialog'
import { Tooltip } from '../../../components/overlay/Tooltip'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { useModelSettings } from '../../../config/ModelSettingsProvider'
import type { AppProject } from '../../../config/projectConfig'
import { AgentAvatar } from '../../agentCollaboration/AgentAvatar'
import type { ChatComposerDraft, ChatConversation } from '../../chat/chatTypes'
import { getChatPermissionPresentation } from '../../chat/chatPermissionPresentation'
import { formatModelConfigLabel } from '../../modelSelection/modelConfigPresentation'
import { requestWorkflows } from '../workflowClient'
import { workflowErrorDetail } from '../workflowErrors'
import { workflowNodeModelLabel } from '../workflowModelPresentation'
import { workflowText } from '../workflowText'
import { WorkflowBindingCanvas } from './WorkflowBindingCanvas'
import {
  projectWorkflowText,
  WORKFLOW_COLORS,
  WORKFLOW_CONVERSATION_DRAG_TYPE
} from './projectWorkflowText'
import '../workflows.css'
import './projectWorkflows.css'

export interface WorkflowsPageProps {
  conversations: readonly ChatConversation[]
  conversationDrafts?: Readonly<Record<string, ChatComposerDraft>>
  projects: readonly AppProject[]
  onNewTemplate: () => void
  onClose?: () => void
  onCommitted: (response: WorkflowResponse) => void | Promise<void>
  onBeforeCommit?: (conversationIds: readonly string[]) => Promise<void>
  onDirtyChange?: (dirty: boolean) => void
  onBusyChange?: (busy: boolean) => void
}

interface BindingDraft {
  id: string
  name: string
  color: string
  record: WorkflowRecord
  instance: WorkflowInstance | null
  bindings: Record<string, string | null>
  baseline: string
}

const draftKey = (draft: Pick<BindingDraft, 'name' | 'color' | 'bindings'>) =>
  JSON.stringify([
    draft.name,
    draft.color,
    Object.entries(draft.bindings).sort(([a], [b]) => a.localeCompare(b))
  ])

export function WorkflowsPage({
  conversations,
  conversationDrafts,
  projects,
  onNewTemplate,
  onClose,
  onCommitted,
  onBeforeCommit,
  onDirtyChange,
  onBusyChange
}: WorkflowsPageProps) {
  const { language } = useFrontendConfig()
  const { models } = useModelSettings()
  const t = useMemo(() => projectWorkflowText(language), [language])
  const text = workflowText(language)
  const [records, setRecords] = useState<WorkflowRecord[]>([])
  const [instances, setInstances] = useState<WorkflowInstance[]>([])
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
  const [choosingTemplate, setChoosingTemplate] = useState(false)
  const [draft, setDraft] = useState<BindingDraft | null>(null)
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null)
  const [search, setSearch] = useState('')
  const [pendingDelete, setPendingDelete] = useState<WorkflowInstance | null>(null)
  const [confirmDiscard, setConfirmDiscard] = useState(false)
  const [versionNotice, setVersionNotice] = useState('')
  const [pendingSync, setPendingSync] = useState<WorkflowResponse | null>(null)
  const requestGeneration = useRef(0)
  const dirty = !!draft && (!draft.instance || draftKey(draft) !== draft.baseline)

  useEffect(() => {
    onDirtyChange?.(dirty)
  }, [dirty, onDirtyChange])
  useEffect(() => {
    onBusyChange?.(busy)
  }, [busy, onBusyChange])
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
      setInstances(workflows.instances ?? [])
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
      if (busyRef.current || pendingSync) return
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

  const agents = draft?.record.definition.nodes.filter((node) => node.kind === 'agent') ?? []
  const selectedNode = agents.find((node) => node.id === selectedNodeId)
  const savedBindings = useMemo(
    () =>
      Object.fromEntries(
        draft?.instance?.bindings.map((binding) => [binding.nodeId, binding.conversationId]) ?? []
      ),
    [draft?.instance]
  )
  const keepsSelectedConversationSettings =
    !!selectedNode &&
    !!draft?.bindings[selectedNode.id] &&
    draft.bindings[selectedNode.id] === savedBindings[selectedNode.id]
  const PermissionIcon = selectedNode
    ? getChatPermissionPresentation(selectedNode.permissionMode).icon
    : null
  const activeConversations = useMemo(
    () => conversations.filter((item) => !item.archivedAt && !item.pendingArchivedAt),
    [conversations]
  )
  const bindingsCount = agents.filter((node) => draft?.bindings[node.id]).length
  const readOnly = !!draft?.instance?.running

  const beginBinding = (record: WorkflowRecord, instance: WorkflowInstance | null = null) => {
    if (pendingSync || busyRef.current) return
    const existing = new Map(
      instance?.bindings.map((binding) => [binding.nodeId, binding.conversationId])
    )
    const bindings = Object.fromEntries(
      record.definition.nodes
        .filter((node) => node.kind === 'agent')
        .map((node) => [node.id, existing.get(node.id) ?? null])
    )
    const color =
      instance?.color ??
      WORKFLOW_COLORS.find((item) => !instances.some((workflow) => workflow.color === item)) ??
      WORKFLOW_COLORS[instances.length % WORKFLOW_COLORS.length]
    const next = {
      id: instance?.id ?? crypto.randomUUID(),
      name: instance?.name ?? record.definition.name,
      color,
      record,
      instance,
      bindings,
      baseline: ''
    }
    next.baseline = draftKey(next)
    setDraft(next)
    setSelectedNodeId(record.definition.nodes.find((node) => node.kind === 'agent')?.id ?? null)
    setSearch('')
    setError('')
    setVersionNotice('')
    setChoosingTemplate(false)
  }

  const refreshVersion = async () => {
    if (!draft || busyRef.current) return
    busyRef.current = true
    setBusy(true)
    try {
      const [templates, workflows] = await Promise.all([
        requestWorkflows({ operation: 'list' }),
        requestWorkflows({ operation: 'listInstances' })
      ])
      const record = templates.records.find(
        (item) => item.definition.id === draft.record.definition.id
      )
      const instance = draft.instance
        ? workflows.instances?.find((item) => item.id === draft.id)
        : null
      setRecords(templates.records)
      setInstances(workflows.instances ?? [])
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
    if (other) return `${t('已加入', 'Assigned to')} ${other.name}`
    const sibling = agents.find(
      (node) => node.id !== nodeId && draft?.bindings[node.id] === conversationId
    )
    return sibling ? `${t('已绑定', 'Bound to')} ${sibling.name}` : null
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
      setError(occupied)
      return
    }
    setError('')
    setSelectedNodeId(nodeId)
    setDraft({ ...draft, bindings: { ...draft.bindings, [nodeId]: conversationId } })
  }

  const acceptCommit = async (response: WorkflowResponse) => {
    setInstances(response.instances ?? [])
    setRecords(response.records)
    setDraft(null)
    setSelectedNodeId(null)
    setPendingDelete(null)
    setError('')
    onDirtyChange?.(false)
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
    if (!draft || busyRef.current || readOnly || !draft.name.trim()) return
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
    if (!pendingDelete || busyRef.current) return
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

  const modelName = (conversation: ChatConversation) => {
    const modelId = conversationDrafts?.[conversation.id]?.modelId ?? conversation.modelId
    const model = models.find((candidate) => candidate.id === modelId)
    return model ? formatModelConfigLabel(model) : t('未选择模型', 'No model selected')
  }
  const projectName = (conversation: ChatConversation) =>
    projects.find((project) => project.id === conversation.projectId)?.name ??
    t('未分配项目', 'No project')
  const candidates = activeConversations.filter((conversation) =>
    `${conversation.title} ${projectName(conversation)}`
      .toLowerCase()
      .includes(search.toLowerCase())
  )
  const back = () => {
    if (busyRef.current) return
    if (draft) {
      if (dirty) {
        setConfirmDiscard(true)
        return
      }
      setDraft(null)
      setSelectedNodeId(null)
      setError('')
    } else if (choosingTemplate) setChoosingTemplate(false)
    else onClose?.()
  }

  return (
    <section className="project-workflows" aria-label={t('工作流管理', 'Workflow management')}>
      <header className="project-workflows__header">
        <div className="project-workflows__identity">
          {(draft || choosingTemplate || onClose) && (
            <Tooltip content={t('返回', 'Back')}>
              <button
                type="button"
                className="workflow-icon-button"
                aria-label={t('返回', 'Back')}
                disabled={busy}
                onClick={back}
              >
                <ArrowLeft />
              </button>
            </Tooltip>
          )}
          <GitBranch aria-hidden="true" />
          <h1>
            {draft
              ? draft.instance
                ? t('管理工作流', 'Manage workflow')
                : t('绑定对话', 'Bind conversations')
              : choosingTemplate
                ? t('选择工作流模板', 'Choose a workflow template')
                : t('工作流', 'Workflows')}
          </h1>
          {draft && (
            <>
              <ChevronRight aria-hidden="true" />
              <span className="project-workflows__template-name">
                {draft.record.definition.name}
              </span>
            </>
          )}
        </div>
        {!draft && (
          <div className="project-workflows__actions">
            <button
              type="button"
              className="workflow-button"
              disabled={busy || !!pendingSync}
              onClick={onNewTemplate}
            >
              <Plus />
              {t('新建模板', 'New template')}
            </button>
            {!choosingTemplate && (
              <button
                type="button"
                className="workflow-button workflow-button--primary"
                disabled={loading || !hasSnapshot || busy || !!pendingSync}
                onClick={() => {
                  setChoosingTemplate(true)
                  setError('')
                }}
              >
                <GitBranch />
                {t('激活工作流', 'Activate workflow')}
              </button>
            )}
          </div>
        )}
      </header>
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
              disabled={busy}
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
            disabled={busy}
            onClick={() => void refreshVersion()}
          >
            <RefreshCw />
          </button>
        </div>
      )}
      {loading ? (
        <div className="project-workflows__empty" role="status">
          {t('正在加载工作流…', 'Loading workflows…')}
        </div>
      ) : !hasSnapshot ? (
        <div className="project-workflows__empty">
          <GitBranch />
          <p>{t('暂时无法读取工作流', 'Workflows could not be loaded')}</p>
        </div>
      ) : draft ? (
        <>
          <div className="project-workflows__binding-toolbar">
            <label>
              <span>{t('名称', 'Name')}</span>
              <input
                value={draft.name}
                maxLength={160}
                disabled={busy || readOnly}
                onChange={(event) => setDraft({ ...draft, name: event.target.value })}
              />
            </label>
            <fieldset className="project-workflows__colors" disabled={busy || readOnly}>
              <legend>{t('颜色', 'Color')}</legend>
              {WORKFLOW_COLORS.map((color, index) => (
                <button
                  key={color}
                  type="button"
                  aria-label={`${t('标记颜色', 'Marker color')} ${index + 1}`}
                  aria-pressed={draft.color === color}
                  style={{ background: color }}
                  onClick={() => setDraft({ ...draft, color })}
                >
                  {draft.color === color && <Check />}
                </button>
              ))}
            </fieldset>
            <span className="project-workflows__binding-help">
              {t(
                '从左侧拖入对话，或点击节点选择',
                'Drag conversations from the sidebar, or select a node'
              )}
            </span>
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
            <WorkflowBindingCanvas
              graph={draft.record.definition}
              conversations={conversations}
              conversationDrafts={conversationDrafts}
              models={models}
              bindings={draft.bindings}
              savedBindings={savedBindings}
              selectedNodeId={selectedNodeId}
              disabled={busy || readOnly}
              onSelect={(id) => {
                setSelectedNodeId(id)
                setSearch('')
              }}
              onBind={bind}
            />
            {selectedNode && (
              <aside
                className="project-workflows__chooser"
                aria-label={t('选择对话', 'Choose a conversation')}
              >
                <div className="project-workflows__chooser-heading">
                  <AgentAvatar agentId={selectedNode.id} />
                  <h2>{selectedNode.name}</h2>
                  <Tooltip content={t('收起', 'Collapse')}>
                    <button
                      type="button"
                      className="workflow-icon-button"
                      aria-label={t('收起', 'Collapse')}
                      onClick={() => setSelectedNodeId(null)}
                    >
                      <X />
                    </button>
                  </Tooltip>
                </div>
                {selectedNode.task && (
                  <p className="project-workflows__node-task">{selectedNode.task}</p>
                )}
                {PermissionIcon && (
                  <div className="project-workflows__permission">
                    <PermissionIcon />
                    <span>
                      {t('节点预设：', 'Node preset: ')}
                      {selectedNode.permissionMode === 'full'
                        ? t('完全权限', 'Full permissions')
                        : selectedNode.permissionMode === 'custom'
                          ? t('自定义权限', 'Custom permissions')
                          : t('默认权限', 'Default permissions')}
                    </span>
                  </div>
                )}
                <p className="project-workflows__permission-note">
                  {keepsSelectedConversationSettings
                    ? t(
                        '当前绑定保持不变，将保留对话现有的模型和权限。',
                        'This binding is unchanged. The conversation keeps its current model and permissions.'
                      )
                    : t(
                        '确认后使用节点预设的模型和权限；正在运行的轮次保持原设置，下轮生效。之后可在对话里独立调整。',
                        'Confirmation applies the node model and permissions once. Active turns keep their settings; changes apply on the next turn. You can adjust conversation settings independently afterward.'
                      )}
                </p>
                <button
                  className={`project-workflows__conversation project-workflows__conversation--new${!draft.bindings[selectedNode.id] ? ' is-selected' : ''}`}
                  type="button"
                  disabled={busy || readOnly}
                  onClick={() => bind(selectedNode.id, null)}
                  aria-pressed={!draft.bindings[selectedNode.id]}
                >
                  <Plus />
                  <span>
                    <strong>{t('自动新建对话', 'Create a new conversation')}</strong>
                    <small>{workflowNodeModelLabel(selectedNode, models, text)}</small>
                  </span>
                  {!draft.bindings[selectedNode.id] && <Check />}
                </button>
                {!!draft.bindings[selectedNode.id] && (
                  <button
                    type="button"
                    className="project-workflows__unbind"
                    disabled={busy || readOnly}
                    onClick={() => bind(selectedNode.id, null)}
                  >
                    <Unlink />
                    {t('解除绑定，改为新建', 'Unbind and create a new conversation')}
                  </button>
                )}
                <label className="project-workflows__search">
                  <Search />
                  <input
                    aria-label={t('搜索对话', 'Search conversations')}
                    placeholder={t('搜索对话或项目', 'Search conversations or projects')}
                    value={search}
                    onChange={(event) => setSearch(event.target.value)}
                  />
                </label>
                <div className="project-workflows__conversation-list">
                  {candidates.map((conversation) => {
                    const assigned = occupancy(conversation.id, selectedNode.id)
                    const selected = draft.bindings[selectedNode.id] === conversation.id
                    return (
                      <button
                        key={conversation.id}
                        type="button"
                        className={`project-workflows__conversation${selected ? ' is-selected' : ''}`}
                        aria-pressed={selected}
                        disabled={busy || readOnly || !!assigned}
                        draggable={!assigned && !busy && !readOnly}
                        onDragStart={(event) => {
                          event.dataTransfer.setData(
                            WORKFLOW_CONVERSATION_DRAG_TYPE,
                            conversation.id
                          )
                          event.dataTransfer.effectAllowed = 'link'
                        }}
                        onClick={() => bind(selectedNode.id, conversation.id)}
                      >
                        <span>
                          <strong>{conversation.title}</strong>
                          <small>
                            {projectName(conversation)} · {modelName(conversation)}
                          </small>
                          {assigned && <small>{assigned}</small>}
                        </span>
                        {selected && <Check />}
                      </button>
                    )
                  })}
                  {!candidates.length && (
                    <p className="project-workflows__hint">
                      {t(
                        '没有找到对话，可在确认时自动新建。',
                        'No conversations found. You can create one when confirming.'
                      )}
                    </p>
                  )}
                </div>
              </aside>
            )}
          </div>
          <footer className="project-workflows__footer">
            <span>
              {t(
                `已绑定 ${bindingsCount} 个 · 将新建 ${agents.length - bindingsCount} 个对话`,
                `${bindingsCount} bound · ${agents.length - bindingsCount} conversations to create`
              )}
            </span>
            <div>
              <button type="button" className="workflow-button" disabled={busy} onClick={back}>
                {t('取消', 'Cancel')}
              </button>
              <button
                type="button"
                className="workflow-button workflow-button--primary"
                disabled={busy || readOnly || !draft.name.trim() || draft.record.issues.length > 0}
                onClick={() => void commit()}
              >
                {busy
                  ? t('正在保存…', 'Saving…')
                  : draft.instance
                    ? t('确认配置', 'Confirm configuration')
                    : t('确认激活', 'Confirm activation')}
              </button>
            </div>
          </footer>
        </>
      ) : choosingTemplate ? (
        <div className="project-workflows__library">
          <p className="project-workflows__hint">
            {t(
              '选择已启用的模板，为每个智能体安排对话。',
              'Choose an enabled template and assign conversations to its agents.'
            )}
          </p>
          {records.map((record) => (
            <button
              key={record.definition.id}
              type="button"
              className="project-workflows__template"
              disabled={!record.enabled || record.issues.length > 0}
              onClick={() => beginBinding(record)}
            >
              <GitBranch />
              <span>
                <strong>{record.definition.name}</strong>
                <small>
                  {record.definition.description || t('工作流模板', 'Workflow template')}
                </small>
                {(!record.enabled || record.issues.length > 0) && (
                  <small>
                    {record.issues.length
                      ? t('需要先完成模板配置', 'Complete the template first')
                      : t('模板尚未启用', 'Template is disabled')}
                  </small>
                )}
              </span>
              <ChevronRight />
            </button>
          ))}
          {!records.length && (
            <div className="project-workflows__empty">
              <GitBranch />
              <p>{t('还没有工作流模板', 'No workflow templates yet')}</p>
              <button
                type="button"
                className="workflow-button"
                disabled={busy || !!pendingSync}
                onClick={onNewTemplate}
              >
                <Plus />
                {t('新建模板', 'New template')}
              </button>
            </div>
          )}
        </div>
      ) : (
        <div className="project-workflows__library">
          {instances.map((instance) => {
            const record = records.find((item) => item.definition.id === instance.templateId)
            return (
              <article className="project-workflows__instance" key={instance.id}>
                <span
                  className="project-workflows__color-dot"
                  style={{ background: instance.color }}
                />
                <button
                  type="button"
                  className="project-workflows__instance-main"
                  disabled={!record || !!pendingSync}
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
                <span
                  className={`project-workflows__status${instance.needsReview ? ' needs-review' : ''}`}
                >
                  {instance.running
                    ? t('运行中', 'Running')
                    : instance.needsReview
                      ? t('需要重新确认', 'Needs review')
                      : t('已配置', 'Configured')}
                </span>
                <Tooltip content={t('移除工作流', 'Remove workflow')}>
                  <button
                    type="button"
                    className="workflow-icon-button"
                    aria-label={`${t('移除工作流', 'Remove workflow')} ${instance.name}`}
                    disabled={busy || instance.running || !!pendingSync}
                    onClick={() => setPendingDelete(instance)}
                  >
                    <Trash2 />
                  </button>
                </Tooltip>
                <Tooltip content={t('管理工作流', 'Manage workflow')}>
                  <button
                    type="button"
                    className="workflow-icon-button"
                    aria-label={`${t('管理工作流', 'Manage workflow')} ${instance.name}`}
                    disabled={!record || !!pendingSync}
                    onClick={() => record && beginBinding(record, instance)}
                  >
                    <ChevronRight />
                  </button>
                </Tooltip>
              </article>
            )
          })}
          {!instances.length && (
            <div className="project-workflows__empty">
              <GitBranch />
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
                onClick={() => setChoosingTemplate(true)}
              >
                <Plus />
                {t('激活工作流', 'Activate workflow')}
              </button>
            </div>
          )}
        </div>
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
          onCancel={() => setConfirmDiscard(false)}
          onConfirm={() => {
            setConfirmDiscard(false)
            setDraft(null)
            setSelectedNodeId(null)
            setError('')
            setVersionNotice('')
          }}
        />
      )}
    </section>
  )
}
