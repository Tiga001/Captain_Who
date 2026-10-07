import type {
  WorkflowInstance,
  WorkflowRecord,
  WorkflowResponse,
  WorkflowIssue
} from '@mycopilot/protocol'
import { ArrowLeft, List, Network, Settings2, RefreshCw, Trash2, Undo2, Redo2 } from 'lucide-react'
import {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties
} from 'react'
import { ConfirmationDialog } from '../../../components/dialog/ConfirmationDialog'
import { Tooltip } from '../../../components/overlay/Tooltip'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { useModelSettings } from '../../../config/ModelSettingsProvider'
import { useAccountAuth } from '../../auth/AccountAuthContext'
import type { AppProject } from '../../../config/projectConfig'
import type { ChatComposerDraft, ChatConversation } from '../../chat/chatTypes'
import type { ConversationAttentionById } from '../../chat/useConversationAttention'
import { SettingsSelect } from '../../settings/components/SettingsSelect'
import { requestWorkflows } from '../workflowClient'
import {
  getCachedWorkflowPage,
  invalidateWorkflowPages,
  isCurrentWorkflowPage,
  setWorkflowPageAuthScope,
  readWorkflowPage
} from '../workflowPageCache'
import { workflowErrorDetail, workflowUnavailableMemberModels } from '../workflowErrors'
import { WorkflowIssues } from '../WorkflowIssues'
import { WorkflowGraphEditor } from '../WorkflowGraphEditor'
import { useWorkflowHistory } from '../useWorkflowHistory'
import { mergeWorkflowDefinition } from '../workflowDefinitionMerge'
import { workflowContentKey } from '../workflowHistory'
import { workflowText } from '../workflowText'
import { hostClient } from '../../../host/hostClient'
import { WorkflowColorPicker } from './WorkflowColorPicker'
import { WorkflowMonitorPage } from './WorkflowMonitorPage'
import { useWorkflowActivity } from './useWorkflowActivity'
import { WorkflowActivityElapsed } from './WorkflowActivityElapsed'
import { projectWorkflowText, pickUnusedWorkflowColor } from './projectWorkflowText'
import '../workflows.css'
import './projectWorkflows.css'

export interface WorkflowsPageProps {
  foreground?: boolean
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
  onTitleChange?: (title: string) => void
  onOpenConversation?: (conversationId: string) => void
}

interface BindingDraft {
  id: string
  name: string
  projectId: string | null
  color: string
  record: WorkflowRecord | null
  instance: WorkflowInstance | null
  bindings: Record<string, string | null>
  baseline: string
}

const draftKey = (
  draft: Pick<BindingDraft, 'name' | 'projectId' | 'color' | 'bindings' | 'record'>
) =>
  JSON.stringify([
    draft.record?.definition.id ?? null,
    draft.name,
    draft.projectId,
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

export function WorkflowsPage(props: WorkflowsPageProps) {
  const auth = useAccountAuth()
  const scope = auth ? `${auth.state.status}:${auth.state.profile?.userId ?? ''}` : 'local-host'
  // Readable snapshots live longer than a navigation key, but never longer than their
  // account session. Remount local editor/loading state as well as clearing the cache.
  setWorkflowPageAuthScope(scope)
  return <WorkflowPageContent key={scope} {...props} />
}

function WorkflowPageContent({
  foreground = true,
  conversations,
  conversationAttention,
  conversationDrafts,
  projects,
  onManageTemplates,
  onClose,
  onCommitted,
  onBeforeCommit,
  onDirtyChange,
  onBusyChange,
  initialMonitorId,
  onMonitorChange,
  onTitleChange,
  onOpenConversation
}: WorkflowsPageProps) {
  const { language } = useFrontendConfig()
  const { models } = useModelSettings()
  const text = workflowText(language)
  const editorId = useId()
  const [editorTab, setEditorTab] = useState<'details' | 'structure'>('structure')
  const t = useMemo(() => projectWorkflowText(language), [language])
  const [initialSnapshot] = useState(() => getCachedWorkflowPage(initialMonitorId ?? null))
  const [records, setRecords] = useState<WorkflowRecord[]>(initialSnapshot?.records ?? [])
  const [instances, setInstances] = useState<WorkflowInstance[]>(initialSnapshot?.instances ?? [])
  const [monitorId, setMonitorId] = useState<string | null>(initialMonitorId ?? null)
  useEffect(() => {
    setMonitorId(initialMonitorId ?? null)
  }, [initialMonitorId])
  const openMonitor = (id: string | null) => {
    setMonitorId(id)
    onMonitorChange?.(id)
  }
  const { runningInstanceIds, activityByInstanceId } = useWorkflowActivity(
    monitorId ? [] : instances,
    conversations,
    foreground
  )
  const [loading, setLoading] = useState(!initialSnapshot)
  const [hasSnapshot, setHasSnapshot] = useState(!!initialSnapshot)
  const [error, setErrorMessage] = useState('')
  const [errorKind, setErrorKind] = useState<'models_unavailable' | null>(null)
  const [errorAcknowledged, setErrorAcknowledged] = useState(false)
  const setError = useCallback((message: string, kind: 'models_unavailable' | null = null) => {
    setErrorMessage(message)
    setErrorKind(kind)
    setErrorAcknowledged(false)
  }, [])
  const [validationIssues, setValidationIssues] = useState<WorkflowIssue[]>([])
  const saveButtonRef = useRef<HTMLButtonElement>(null)
  const [busy, setBusy] = useState(false)
  const busyRef = useRef(false)
  // Transient switch locks use aria-disabled plus handler guards to preserve appearance and focus.
  const [togglingId, setTogglingId] = useState<string | null>(null)
  const togglingIdRef = useRef<string | null>(null)
  const [draft, setDraft] = useState<BindingDraft | null>(null)
  const history = useWorkflowHistory()
  const graph = history.draft
  const [graphBaseline, setGraphBaseline] = useState('')
  const [pendingDelete, setPendingDelete] = useState<WorkflowInstance | null>(null)
  const [confirmDiscard, setConfirmDiscard] = useState(false)
  const [pendingTemplate, setPendingTemplate] = useState<WorkflowRecord | null>(null)
  const [mergeNotice, setMergeNotice] = useState('')
  const [bindingNotice, setBindingNotice] = useState('')
  const [pendingSync, setPendingSync] = useState<WorkflowResponse | null>(null)
  const requestGeneration = useRef(0)
  const managementVersions = useRef(new Map<string, number>())
  const pendingManagementVersions = useRef(new Map<string, number>())
  const pendingOrganizationRefresh = useRef(false)
  const foregroundRef = useRef(foreground)
  useLayoutEffect(() => {
    foregroundRef.current = foreground
  }, [foreground])
  const dirty =
    !!draft &&
    (draftKey(draft) !== draft.baseline || (!!graph && workflowContentKey(graph) !== graphBaseline))
  const pageTitle = monitorId
    ? (instances.find((instance) => instance.id === monitorId)?.name ?? t('组织', 'Organizations'))
    : draft?.name.trim() || t('组织', 'Organizations')

  useEffect(() => {
    onTitleChange?.(pageTitle)
  }, [onTitleChange, pageTitle])

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
    const generation = requestGeneration.current
    setLoading(true)
    setError('')
    try {
      // Cached content paints immediately, but closing a tab can miss structural
      // notifications. Always revalidate on navigation without blanking the board.
      const templates = await readWorkflowPage(monitorId, requestWorkflows, true)
      if (generation !== requestGeneration.current) return
      setRecords(templates.records)
      setInstances((current) => keepInstanceOrder(current, templates.instances ?? []))
      setHasSnapshot(true)
    } catch {
      if (generation === requestGeneration.current)
        setError(
          language === 'zh-CN'
            ? '组织加载失败，请重试。'
            : 'Could not load organizations. Please retry.'
        )
    } finally {
      if (generation === requestGeneration.current) setLoading(false)
    }
  }, [language, monitorId, setError, setLoading, setRecords, setInstances, setHasSnapshot])

  useEffect(() => {
    void reload()
    return () => {
      requestGeneration.current += 1
    }
  }, [reload])

  const editorSnapshot = useRef({ draft, graph, pendingSync })
  const synchronizeVersionRef = useRef<() => Promise<'applied' | 'deferred' | 'failed'>>(
    async () => 'deferred'
  )
  const flushRefreshRef = useRef<() => void>(() => undefined)
  useLayoutEffect(() => {
    editorSnapshot.current = { draft, graph, pendingSync }
  }, [draft, graph, pendingSync])

  useEffect(() => {
    let stopped = false
    let reading = false
    let retryTimer: ReturnType<typeof setTimeout> | undefined
    let failures = 0
    const flush = async () => {
      if (
        stopped ||
        !foregroundRef.current ||
        reading ||
        !pendingOrganizationRefresh.current ||
        busyRef.current ||
        togglingIdRef.current ||
        editorSnapshot.current.pendingSync
      )
        return
      clearTimeout(retryTimer)
      pendingOrganizationRefresh.current = false
      reading = true
      const result = await synchronizeVersionRef.current()
      reading = false
      if (stopped) return
      if (result === 'applied') {
        failures = 0
        // readWorkflowPage folds invalidations received during the flight into its
        // result. They do not require a third, identical read after it completes.
        pendingOrganizationRefresh.current = false
        for (const [id, sequence] of pendingManagementVersions.current) {
          managementVersions.current.set(id, sequence)
        }
        pendingManagementVersions.current.clear()
      } else {
        pendingOrganizationRefresh.current = true
        // Retry failed reads without spinning or interrupting editing.
        const delay = result === 'failed' ? Math.min(1000 * 2 ** Math.min(failures++, 5), 30000) : 0
        retryTimer = setTimeout(() => void flush(), delay)
      }
    }
    const focused = () => {
      pendingOrganizationRefresh.current = true
      void flush()
    }
    const changed = () => {
      invalidateWorkflowPages()
      focused()
    }
    flushRefreshRef.current = () => void flush()
    window.addEventListener('captain:workflows-changed', changed)
    window.addEventListener('focus', focused)
    const unsubscribe = hostClient.agent.onWorkflowRuntimeChanged?.((snapshot) => {
      const sequence = Math.max(
        snapshot.summary?.structureRevision ?? 0,
        ...snapshot.events
          .filter((event) => event.kind === 'members_changed')
          .map((event) => event.sequence)
      )
      if (
        sequence <=
        Math.max(
          managementVersions.current.get(snapshot.instanceId) ?? 0,
          pendingManagementVersions.current.get(snapshot.instanceId) ?? 0
        )
      )
        return
      pendingManagementVersions.current.set(snapshot.instanceId, sequence)
      changed()
    })
    return () => {
      stopped = true
      clearTimeout(retryTimer)
      flushRefreshRef.current = () => undefined
      window.removeEventListener('captain:workflows-changed', changed)
      window.removeEventListener('focus', focused)
      unsubscribe?.()
    }
  }, [])
  useEffect(() => {
    flushRefreshRef.current()
  }, [busy, togglingId, pendingSync, foreground])

  const agents = graph?.nodes.filter((node) => node.kind === 'agent') ?? []
  const readyRecords = records.filter((record) => record.enabled && record.issues.length === 0)
  const activeConversations = useMemo(
    () => conversations.filter((item) => !item.archivedAt && !item.pendingArchivedAt),
    [conversations]
  )
  const projectMissing = !!draft?.projectId && !projects.some((item) => item.id === draft.projectId)
  const unavailableColors = instances
    .filter((instance) => instance.enabled && instance.id !== draft?.id)
    .map((instance) => instance.color)
  const colorOccupied =
    !!draft &&
    draft.instance?.enabled !== false &&
    unavailableColors.some((color) => color.toLowerCase() === draft.color.toLowerCase())
  const noColorsMessage = t(
    '可选颜色已全部被占用，请先停用或调整其他组织。',
    'All available colors are in use. Disable or adjust another organization first.'
  )
  const colorOccupiedMessage = t(
    '这个颜色已被其他组织使用，请选择其他颜色。',
    'This color is used by another organization. Choose a different color.'
  )

  const beginBinding = (
    record: WorkflowRecord | null,
    instance: WorkflowInstance | null = null
  ) => {
    if (pendingSync || busyRef.current || (instance && togglingIdRef.current === instance.id))
      return
    const definition = instance?.definition ?? record?.definition ?? null
    const existing = new Map(
      instance?.bindings.map((binding) => [binding.nodeId, binding.conversationId])
    )
    const bindings = Object.fromEntries(
      (definition?.nodes ?? [])
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
      projectId: instance?.projectId ?? null,
      color,
      record,
      instance,
      bindings,
      baseline: ''
    }
    next.baseline = draftKey(next)
    setDraft(next)
    setEditorTab('structure')
    history.reset(definition ? structuredClone(definition) : null)
    setGraphBaseline(definition ? workflowContentKey(definition) : '')
    setError('')
    setMergeNotice('')
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
    setEditorTab('structure')
    history.reset(structuredClone(record.definition))
    setGraphBaseline(workflowContentKey(record.definition))
    setError('')
    setMergeNotice('')
  }

  const requestTemplate = (templateId: string) => {
    const record = readyRecords.find((item) => item.definition.id === templateId)
    if (!record || draft?.record?.definition.id === templateId) return
    if (dirty) {
      setPendingTemplate(record)
      setConfirmDiscard(true)
    } else selectTemplate(record)
  }

  const synchronizeVersion = async (): Promise<'applied' | 'deferred' | 'failed'> => {
    const generation = requestGeneration.current
    try {
      const templates = await readWorkflowPage(monitorId, requestWorkflows, true)
      if (
        generation !== requestGeneration.current ||
        !isCurrentWorkflowPage(monitorId, templates) ||
        busyRef.current ||
        togglingIdRef.current ||
        editorSnapshot.current.pendingSync
      )
        return 'deferred'
      setRecords(templates.records)
      setInstances((current) => keepInstanceOrder(current, templates.instances ?? []))
      setHasSnapshot(true)
      setLoading(false)
      // Use the draft at response time so in-flight local edits survive the merge.
      const { draft, graph } = editorSnapshot.current
      if (!draft || (!draft.instance && !draft.record)) return 'applied'
      const record = templates.records.find(
        (item) => item.definition.id === draft.record?.definition.id
      )
      const instance = draft.instance
        ? templates.instances?.find((item) => item.id === draft.id)
        : null
      if ((draft.instance && !instance) || (!draft.instance && !record)) {
        setError(
          t(
            '这个模板或组织已被移除。请返回列表重新选择。',
            'This template or organization was removed. Return to the list and choose another.'
          )
        )
        return 'applied'
      }
      // Other organizations and ordinary runtime events must not clear the undo history.
      if (
        draft.instance
          ? instance?.revision === draft.instance.revision
          : record?.revision === draft.record?.revision
      )
        return 'applied'
      const latest = instance?.definition ?? record!.definition
      const baseline = draft.instance?.definition ?? draft.record?.definition
      const merged =
        graph && baseline
          ? mergeWorkflowDefinition(baseline, graph, latest)
          : { definition: latest, removedEntities: [] }
      const { definition, removedEntities } = merged
      const prior = new Map(
        instance?.bindings.map((binding) => [binding.nodeId, binding.conversationId])
      )
      const bindings = Object.fromEntries(
        definition.nodes
          .filter((node) => node.kind === 'agent')
          .map((node) => [
            node.id,
            (draft.bindings[node.id] ?? null) !==
            (draft.instance?.bindings.find((binding) => binding.nodeId === node.id)
              ?.conversationId ?? null)
              ? (draft.bindings[node.id] ?? null)
              : (prior.get(node.id) ?? null)
          ])
      )
      history.reset(structuredClone(definition))
      setGraphBaseline(workflowContentKey(latest))
      const next = {
        ...draft,
        record: draft.instance ? null : (record ?? null),
        instance: instance ?? null,
        bindings,
        name: instance && draft.name === draft.instance?.name ? instance.name : draft.name,
        projectId:
          instance && draft.projectId === (draft.instance?.projectId ?? null)
            ? (instance.projectId ?? null)
            : draft.projectId,
        color: instance && draft.color === draft.instance?.color ? instance.color : draft.color
      }
      if (instance)
        next.baseline = draftKey({
          ...next,
          name: instance.name,
          projectId: instance.projectId ?? null,
          color: instance.color,
          bindings: Object.fromEntries(
            instance.bindings.map((binding) => [binding.nodeId, binding.conversationId])
          )
        })
      setDraft(next)
      if (removedEntities.length) {
        setMergeNotice(
          t(
            `“${removedEntities.map((item) => item.name).join('”、“')}”已被移除，相关修改未恢复；其余修改已保留，${removedEntities.some((item) => item.kind === 'department') ? '请检查部门归属和管理身份后保存。' : '请检查后保存。'}`,
            `${removedEntities.map((item) => `“${item.name}”`).join(', ')} were removed. Their edits were not restored; your other edits were retained. ${removedEntities.some((item) => item.kind === 'department') ? 'Review department assignments and management roles before saving.' : 'Review before saving.'}`
          )
        )
      }
      return 'applied'
    } catch {
      return 'failed'
    }
  }
  useLayoutEffect(() => {
    synchronizeVersionRef.current = synchronizeVersion
  })

  const occupancy = (conversationId: string, nodeId: string): string | null => {
    const other = instances.find(
      (instance) =>
        instance.id !== draft?.id &&
        instance.bindings.some((binding) => binding.conversationId === conversationId)
    )
    if (other)
      return t(
        `这个对话已经加入其他组织“${other.name}”，不能重复分配。`,
        `This conversation already belongs to another organization, “${other.name}”, and cannot be assigned again.`
      )
    const sibling = agents.find(
      (node) => node.id !== nodeId && draft?.bindings[node.id] === conversationId
    )
    return sibling
      ? t(
          `这个对话已分配给当前组织的“${sibling.name}”，不能重复分配。`,
          `This conversation is already assigned to “${sibling.name}” in this organization and cannot be assigned again.`
        )
      : null
  }

  const bind = (nodeId: string, conversationId: string | null) => {
    if (!draft || busyRef.current) return
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
    setDraft({ ...draft, bindings: { ...draft.bindings, [nodeId]: conversationId } })
  }

  const acceptCommit = async (response: WorkflowResponse) => {
    invalidateWorkflowPages()
    requestGeneration.current += 1
    setLoading(false)
    setInstances((current) => keepInstanceOrder(current, response.instances ?? []))
    setRecords(response.records)
    setDraft(null)
    setMergeNotice('')
    setPendingDelete(null)
    setError('')
    onDirtyChange?.(false)
    await synchronizeCommit(response)
  }

  const synchronizeCommit = async (response: WorkflowResponse) => {
    try {
      await onCommitted(response)
      setPendingSync(null)
    } catch {
      setPendingSync(response)
      setError(
        t(
          '组织已保存，对话列表刷新失败，请重试同步。',
          'The organization was saved, but the conversation list could not refresh. Retry synchronization.'
        )
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
    } catch {
      setError(
        t(
          '同步失败，已保存的组织不会重复创建，请重试。',
          'Synchronization failed. The saved organization will not be created again. Please retry.'
        )
      )
    } finally {
      busyRef.current = false
      setBusy(false)
    }
  }

  const commit = async () => {
    if (
      !draft ||
      !graph ||
      busyRef.current ||
      togglingIdRef.current ||
      pendingSync ||
      projectMissing ||
      !draft.name.trim()
    )
      return
    if (colorOccupied) {
      setError(colorOccupiedMessage)
      return
    }
    requestGeneration.current += 1
    busyRef.current = true
    setBusy(true)
    setError('')
    try {
      const validation = await requestWorkflows({
        operation: 'validate',
        definition: { ...graph, name: draft.name.trim() }
      })
      if (validation.issues.length) {
        setValidationIssues(validation.issues)
        return
      }
      await onBeforeCommit?.(Object.values(draft.bindings).filter((id): id is string => !!id))
      const response = await requestWorkflows({
        operation: 'saveInstance',
        id: draft.id,
        ...(!draft.instance && draft.record
          ? {
              templateId: draft.record.definition.id,
              expectedTemplateRevision: draft.record.revision
            }
          : {}),
        definition: { ...graph, name: draft.name.trim() },
        name: draft.name.trim(),
        projectId: draft.projectId,
        color: draft.color,
        bindings: agents.map((node) => ({
          nodeId: node.id,
          conversationId: draft.bindings[node.id] ?? null
        })),
        expectedRevision: draft.instance?.revision ?? 0
      })
      await acceptCommit(response)
    } catch (cause) {
      if (workflowErrorDetail(cause).includes('organization_duplicate_member_name')) {
        setError(text('duplicateMemberNameHint'))
        return
      }
      if (workflowErrorDetail(cause).includes('organization_duplicate_department_name')) {
        setError(text('duplicateDepartmentNameHint'))
        return
      }
      if (workflowErrorDetail(cause).includes('organization_department_name_separator')) {
        setError(text('departmentNameSeparatorHint'))
        return
      }
      if (workflowErrorDetail(cause).includes('workflow_project_missing')) {
        setError(
          t(
            '所选项目已不存在，请重新选择所属项目。',
            'The selected project no longer exists. Choose a different project.'
          )
        )
        return
      }
      if (workflowErrorDetail(cause).includes('workflow_conversation_already_bound')) {
        setError('')
        setBindingNotice(
          t(
            '这个对话已经加入其他组织，不能重复分配。',
            'This conversation already belongs to another organization and cannot be assigned again.'
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
      if (
        workflowErrorDetail(cause).includes(
          'Organization changed or was deleted; reload before changing it'
        )
      ) {
        pendingOrganizationRefresh.current = true
        setError(
          t(
            '组织信息已更新，正在自动同步并保留你的修改。请检查后再次保存。',
            'The organization changed. We are syncing it and keeping your edits. Review before saving again.'
          )
        )
        return
      }
      setError(
        t(
          '暂时无法保存组织，当前修改已保留。请稍后再保存。',
          'Could not save this organization. Your edits are preserved. Please try saving again later.'
        )
      )
    } finally {
      busyRef.current = false
      setBusy(false)
    }
  }

  const remove = async () => {
    if (!pendingDelete || busyRef.current || togglingIdRef.current) return
    requestGeneration.current += 1
    busyRef.current = true
    setBusy(true)
    try {
      const response = await requestWorkflows({
        operation: 'deleteInstance',
        id: pendingDelete.id,
        expectedRevision: pendingDelete.revision
      })
      await acceptCommit(response)
    } catch {
      pendingOrganizationRefresh.current = true
      setError(
        t(
          '暂时无法移除组织，正在同步最新状态。请稍后重试。',
          'Could not remove this organization. We are syncing its latest state. Please try again later.'
        )
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
      invalidateWorkflowPages()
      setInstances((current) => keepInstanceOrder(current, response.instances ?? []))
      setRecords(response.records)
      await synchronizeCommit(response)
    } catch (cause) {
      pendingOrganizationRefresh.current = true
      const unavailableModels = !instance.enabled ? workflowUnavailableMemberModels(cause) : null
      if (unavailableModels) {
        setError(
          [
            t('成员使用的模型不可用。', 'Models used by organization members are unavailable.'),
            ...unavailableModels.map((model) => {
              const name = model.modelDisplayName
                ? `${model.modelDisplayName}${t('（不可用）', ' (unavailable)')}`
                : t('已删除或不可用的模型', 'Deleted or unavailable model')
              return `${name}\n${t('受影响成员：', 'Affected members: ')}${model.memberNames.join(t('、', ', '))}`
            }),
            t(
              '请在模型设置中检查并启用该模型，或在组织配置中为这些成员选择可用模型。',
              'Check and enable these models in Model settings, or choose available models for these members in Organization settings.'
            )
          ].join('\n\n'),
          'models_unavailable'
        )
        return
      }
      setError(
        workflowErrorDetail(cause).includes('organization_duplicate_member_name')
          ? t(
              '成员名称重复，请打开组织配置并修改。不同部门的成员也不能重名，首尾空格和大小写不用于区分名称。',
              'Member names must be unique. Open organization settings and rename duplicate members, including those in different departments. Letter case and surrounding spaces do not distinguish names.'
            )
          : workflowErrorDetail(cause).includes('workflow_color_in_use')
            ? colorOccupiedMessage
            : t(
                '暂时无法切换组织状态，请检查配置后重试。',
                'Could not change organization state. Check its configuration and try again.'
              )
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
      setMergeNotice('')
      setError('')
    } else onClose?.()
  }

  const errorDialog =
    foreground && error && !errorAcknowledged ? (
      <ConfirmationDialog
        dialogRole="alertdialog"
        title={
          errorKind === 'models_unavailable'
            ? t('无法启用组织', 'Could not enable organization')
            : t('暂时无法完成操作', 'Could not complete this action')
        }
        description={error}
        descriptionClassName={
          errorKind === 'models_unavailable' ? 'project-workflows__model-error' : undefined
        }
        cancelLabel={t('关闭', 'Close')}
        confirmLabel={t('知道了', 'Got it')}
        confirmVariant="primary"
        showCancelButton={false}
        onCancel={() => setErrorAcknowledged(true)}
        onConfirm={() => setErrorAcknowledged(true)}
      />
    ) : null

  if (monitorId) {
    const instance = instances.find((item) => item.id === monitorId)
    if (instance) {
      return (
        <>
          <WorkflowMonitorPage
            foreground={foreground}
            key={instance.id}
            instance={instance}
            conversations={conversations}
            conversationAttention={conversationAttention}
            conversationDrafts={conversationDrafts}
            models={models}
            onBack={() => openMonitor(null)}
            onOpenConversation={(id) => onOpenConversation?.(id)}
          />
          {errorDialog}
        </>
      )
    }
    return (
      <section className="project-workflows" aria-label={t('组织看板', 'Organization board')}>
        <header className="project-workflows__header">
          <div className="project-workflows__identity">
            <Tooltip content={t('返回组织', 'Back to organizations')}>
              <button
                type="button"
                className="workflow-icon-button"
                aria-label={t('返回组织', 'Back to organizations')}
                onClick={() => openMonitor(null)}
              >
                <ArrowLeft aria-hidden="true" />
              </button>
            </Tooltip>
            <h1>{t('组织看板', 'Organization board')}</h1>
          </div>
        </header>
        <div className="project-workflows__empty" role="status">
          <p>
            {loading
              ? t('正在加载组织…', 'Loading organizations…')
              : t('组织已不可用', 'This organization or its template is unavailable')}
          </p>
          {!loading && (
            <button className="workflow-button" type="button" onClick={() => void reload()}>
              <RefreshCw aria-hidden="true" />
              {t('重试', 'Retry')}
            </button>
          )}
        </div>
        {errorDialog}
      </section>
    )
  }

  return (
    <section
      onKeyDown={(event) => {
        if (
          !draft ||
          busy ||
          event.defaultPrevented ||
          event.nativeEvent.isComposing ||
          (!event.metaKey && !event.ctrlKey) ||
          document.querySelector('[aria-modal="true"]')
        )
          return
        const typing = (event.target as Element).closest(
          'input, textarea, select, [contenteditable="true"]'
        )
        if (event.key.toLowerCase() === 's') {
          event.preventDefault()
          void commit()
        } else if (!typing && event.key.toLowerCase() === 'z') {
          event.preventDefault()
          if (event.shiftKey) history.redo()
          else history.undo()
        } else if (!typing && event.key.toLowerCase() === 'y') {
          event.preventDefault()
          history.redo()
        }
      }}
      className={`project-workflows${draft ? ' project-workflows--editing' : ''}`}
      aria-label={t('组织管理', 'Organization management')}
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
            <h1>{t('组织', 'Organizations')}</h1>
          </div>
          <div className="project-workflows__actions">
            <button
              type="button"
              className="workflow-button"
              disabled={busy || !!pendingSync}
              onClick={onManageTemplates}
            >
              <List aria-hidden="true" />
              {t('管理组织模板', 'Manage organization templates')}
            </button>
            <button
              type="button"
              className="workflow-button workflow-button--primary"
              disabled={loading || !hasSnapshot || busy || !!pendingSync}
              onClick={() => beginBinding(null)}
            >
              <Network aria-hidden="true" />
              {t('激活新组织', 'Activate new organization')}
            </button>
          </div>
        </header>
      )}
      {(pendingSync || (error && hasSnapshot && !draft)) && (
        <div className="project-workflows__recovery">
          {pendingSync ? (
            <>
              <span>
                {t(
                  '组织已保存，等待同步对话列表。',
                  'Organization saved. The conversation list needs to sync.'
                )}
              </span>
              <button
                className="workflow-button"
                type="button"
                disabled={busy}
                onClick={() => void retrySync()}
              >
                <RefreshCw aria-hidden="true" />
                {t('重试同步', 'Retry synchronization')}
              </button>
            </>
          ) : (
            <button
              className="workflow-button"
              type="button"
              disabled={busy || loading}
              onClick={() => void reload()}
            >
              <RefreshCw aria-hidden="true" />
              {t('重试', 'Retry')}
            </button>
          )}
        </div>
      )}
      {loading && !hasSnapshot ? (
        <div className="project-workflows__empty" role="status">
          {t('正在加载组织…', 'Loading organizations…')}
        </div>
      ) : !hasSnapshot ? (
        <div className="project-workflows__empty">
          <Network />
          <p>{t('暂时无法读取组织', 'Organizations could not be loaded')}</p>
          <button className="workflow-button" type="button" onClick={() => void reload()}>
            <RefreshCw aria-hidden="true" />
            {t('重试', 'Retry')}
          </button>
        </div>
      ) : draft ? (
        <>
          <div
            className="project-workflows__binding-toolbar"
            role="toolbar"
            aria-label={t('组织配置', 'Organization configuration')}
          >
            {!draft.instance && (
              <div className="project-workflows__template-field">
                <span>{t('模版', 'Template')}</span>
                <SettingsSelect
                  ariaLabel={t('模版', 'Template')}
                  className="project-workflows__template-select"
                  value={draft.record?.definition.id ?? ''}
                  disabled={busy}
                  options={[
                    {
                      value: '',
                      label: t('请选择组织模板', 'Choose an organization template'),
                      disabled: true
                    },
                    ...readyRecords.map((record) => ({
                      value: record.definition.id,
                      label: record.definition.name
                    }))
                  ]}
                  onChange={requestTemplate}
                />
              </div>
            )}
            <label>
              <span>{t('名称', 'Name')}</span>
              <input
                aria-label={t('名称', 'Name')}
                value={draft.name}
                maxLength={160}
                disabled={busy}
                onChange={(event) => setDraft({ ...draft, name: event.target.value })}
              />
            </label>
            <div className="project-workflows__project-field">
              <Tooltip
                content={t(
                  '自动新建的对话将放入这个项目，拖入的已有对话保留原项目。',
                  'New conversations are created in this project. Assigned conversations keep their current project.'
                )}
              >
                <span>{t('所属项目', 'Project')}</span>
              </Tooltip>
              <SettingsSelect
                ariaLabel={t('所属项目', 'Project')}
                className="project-workflows__project-select"
                value={draft.projectId ?? ''}
                disabled={busy}
                options={[
                  { value: '', label: t('无项目', 'No project') },
                  ...projects.map((project) => ({ value: project.id, label: project.name })),
                  ...(projectMissing
                    ? [
                        {
                          value: draft.projectId!,
                          label: t('项目已删除', 'Project deleted'),
                          disabled: true
                        }
                      ]
                    : [])
                ]}
                onChange={(projectId) => setDraft({ ...draft, projectId: projectId || null })}
              />
            </div>
            <div className="project-workflows__editing-actions">
              {graph && (
                <div className="project-workflows__editor-controls">
                  <div
                    className="workflow-page-tabs"
                    role="tablist"
                    aria-label={t('组织配置', 'Organization configuration')}
                  >
                    {(['details', 'structure'] as const).map((tab) => (
                      <button
                        key={tab}
                        type="button"
                        className="workflow-tab"
                        role="tab"
                        id={`${editorId}-${tab}`}
                        aria-controls={`${editorId}-${tab}-panel`}
                        aria-selected={editorTab === tab}
                        tabIndex={editorTab === tab ? 0 : -1}
                        onClick={() => setEditorTab(tab)}
                        onKeyDown={(event) => {
                          if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key))
                            return
                          event.preventDefault()
                          const next =
                            event.key === 'Home'
                              ? 'details'
                              : event.key === 'End'
                                ? 'structure'
                                : tab === 'details'
                                  ? 'structure'
                                  : 'details'
                          setEditorTab(next)
                          document.getElementById(`${editorId}-${next}`)?.focus()
                        }}
                      >
                        {text(tab === 'details' ? 'basicInfo' : 'structureTab')}
                      </button>
                    ))}
                  </div>
                  <div className="workflow-history-actions">
                    <button
                      type="button"
                      className="workflow-graph-icon-button"
                      aria-label={t('撤销', 'Undo')}
                      disabled={busy || !history.canUndo}
                      onClick={history.undo}
                    >
                      <Undo2 />
                    </button>
                    <button
                      type="button"
                      className="workflow-graph-icon-button"
                      aria-label={t('重做', 'Redo')}
                      disabled={busy || !history.canRedo}
                      onClick={history.redo}
                    >
                      <Redo2 />
                    </button>
                  </div>
                </div>
              )}
              <div className="project-workflows__commit-actions">
                <button type="button" className="workflow-button" disabled={busy} onClick={back}>
                  {t('取消', 'Cancel')}
                </button>
                <button
                  type="button"
                  className="workflow-button workflow-button--primary"
                  ref={saveButtonRef}
                  disabled={
                    busy ||
                    !!togglingId ||
                    !!pendingSync ||
                    projectMissing ||
                    colorOccupied ||
                    !graph ||
                    !draft.name.trim()
                  }
                  onClick={() => void commit()}
                >
                  {draft.instance ? t('保存', 'Save') : t('激活', 'Activate')}
                </button>
              </div>
            </div>
          </div>
          <div
            className="project-workflows__binding-content"
            style={{ '--workflow-instance-color': draft.color } as CSSProperties}
          >
            {graph ? (
              <div className="project-workflows__editor">
                <div className="workflow-editing-content">
                  <div
                    className="workflow-details-panel"
                    role="tabpanel"
                    id={`${editorId}-details-panel`}
                    aria-labelledby={`${editorId}-details`}
                    hidden={editorTab !== 'details'}
                  >
                    <form
                      className="workflow-definition-form"
                      onSubmit={(event) => {
                        event.preventDefault()
                        void commit()
                      }}
                    >
                      <fieldset disabled={busy}>
                        <label>
                          <span>{text('description')}</span>
                          <textarea
                            rows={3}
                            maxLength={2048}
                            placeholder={text('descriptionPlaceholder')}
                            value={graph.description}
                            onChange={(event) => {
                              const description = event.currentTarget.value
                              history.change((current) => ({ ...current, description }), {
                                group: 'description'
                              })
                            }}
                          />
                        </label>
                        <label>
                          <span>{text('background')}</span>
                          <textarea
                            rows={7}
                            maxLength={32768}
                            placeholder={text('backgroundPlaceholder')}
                            value={graph.background}
                            onChange={(event) => {
                              const background = event.currentTarget.value
                              history.change((current) => ({ ...current, background }), {
                                group: 'background'
                              })
                            }}
                          />
                        </label>
                      </fieldset>
                    </form>
                  </div>
                  <div
                    className="workflow-structure-panel"
                    role="tabpanel"
                    id={`${editorId}-structure-panel`}
                    aria-labelledby={`${editorId}-structure`}
                    hidden={editorTab !== 'structure'}
                  >
                    <WorkflowGraphEditor
                      key={draft.id}
                      definition={graph}
                      text={workflowText(language)}
                      disabled={busy}
                      onChange={history.change}
                      canvasActions={
                        <WorkflowColorPicker
                          variant="canvas"
                          color={draft.color}
                          unavailableColors={
                            draft.instance?.enabled === false ? [] : unavailableColors
                          }
                          disabled={busy}
                          onChange={(color) => setDraft({ ...draft, color })}
                        />
                      }
                      conversationBindings={{
                        values: draft.bindings,
                        options: activeConversations.map((conversation) => ({
                          id: conversation.id,
                          title: conversation.title
                        })),
                        label: t('绑定对话', 'Bind conversation'),
                        emptyLabel: t('保存时新建对话', 'Create conversation on save'),
                        onChange: bind
                      }}
                    />
                  </div>
                </div>
              </div>
            ) : (
              <div
                className="workflow-canvas-shell workflow-binding-canvas-shell project-workflows__blank-canvas"
                aria-label={t('空白组织画布', 'Empty organization canvas')}
              >
                <div className="project-workflows__empty">
                  <Network aria-hidden="true" />
                  <p>
                    {readyRecords.length
                      ? t('在上方选择组织模板', 'Choose an organization template above')
                      : t('还没有可用的组织模板', 'No organization templates are ready yet')}
                  </p>
                  {!readyRecords.length && (
                    <button type="button" className="workflow-button" onClick={onManageTemplates}>
                      <List aria-hidden="true" />
                      {t('管理组织模板', 'Manage organization templates')}
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
            const complete = instance.definition.nodes
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
            const enableBlocked = !instance.enabled && (!complete || conflicts)
            const switchTitle = instance.enabled
              ? t('停用组织', 'Disable organization')
              : conflicts
                ? colorOccupiedMessage
                : enableBlocked
                  ? t('请先补全成员的对话分配', 'Complete member conversation assignments first')
                  : t('启用组织', 'Enable organization')
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
                  disabled={busy || !!pendingSync}
                  onClick={() => beginBinding(null, instance)}
                >
                  <strong>{instance.name}</strong>
                  <small className="project-workflows__instance-meta">
                    <span>
                      {t(
                        `${instance.bindings.length} 个对话`,
                        `${instance.bindings.length} conversations`
                      )}
                    </span>
                    <WorkflowActivityElapsed
                      activity={activityByInstanceId.get(instance.id)}
                      language={language}
                    />
                  </small>
                </button>
                <Tooltip content={switchTitle}>
                  <button
                    type="button"
                    role="switch"
                    className="project-workflows__switch"
                    aria-label={`${instance.enabled ? t('停用组织', 'Disable organization') : t('启用组织', 'Enable organization')} ${instance.name}`}
                    aria-checked={instance.enabled}
                    aria-busy={togglingId === instance.id || undefined}
                    aria-disabled={!!togglingId || undefined}
                    disabled={busy || !!pendingSync || enableBlocked}
                    onClick={() => void setInstanceEnabled(instance)}
                  >
                    <span aria-hidden="true" />
                  </button>
                </Tooltip>
                <Tooltip content={t('组织看板', 'Organization board')}>
                  <button
                    type="button"
                    className="workflow-icon-button project-workflows__diagram"
                    aria-label={`${t('组织看板', 'Organization board')} ${instance.name}`}
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
                    disabled={busy || !!pendingSync}
                    onClick={() => beginBinding(null, instance)}
                  >
                    <Settings2 aria-hidden="true" />
                  </button>
                </Tooltip>
                <Tooltip content={t('移除组织', 'Remove organization')}>
                  <button
                    type="button"
                    className="workflow-icon-button"
                    aria-label={`${t('移除组织', 'Remove organization')} ${instance.name}`}
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
                  '从模板创建组织，再为智能体绑定对话。',
                  'Create an organization from a template, then bind conversations to its agents.'
                )}
              </p>
              <button
                type="button"
                className="workflow-button workflow-button--primary"
                disabled={busy || !!pendingSync}
                onClick={() => beginBinding(null)}
              >
                <Settings2 aria-hidden="true" />
                {t('激活新组织', 'Activate new organization')}
              </button>
            </div>
          )}
        </div>
      )}
      {errorDialog}
      {foreground && mergeNotice && (
        <ConfirmationDialog
          title={t('部分修改需要检查', 'Review affected edits')}
          description={mergeNotice}
          cancelLabel={t('关闭', 'Close')}
          confirmLabel={t('知道了', 'Got it')}
          confirmVariant="primary"
          showCancelButton={false}
          onCancel={() => setMergeNotice('')}
          onConfirm={() => setMergeNotice('')}
        />
      )}
      {graph && validationIssues.length > 0 && (
        <WorkflowIssues
          graph={graph}
          issues={validationIssues}
          text={text}
          restoreFocusRef={saveButtonRef}
          title={t('请完善组织配置', 'Complete organization configuration')}
          hint={t('补全以下配置后即可保存。', 'Complete these settings before saving.')}
          onClose={() => setValidationIssues([])}
        />
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
          title={t('移除组织？', 'Remove organization?')}
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
          title={t('放弃未保存的修改？', 'Discard unsaved changes?')}
          description={t(
            '这次选择不会应用到对话，已有组织保持不变。',
            'These selections will not be applied to conversations. Existing organizations will remain unchanged.'
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
            setError('')
            setMergeNotice('')
          }}
        />
      )}
    </section>
  )
}
