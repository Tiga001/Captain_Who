import type { WorkflowRecord } from '@mycopilot/protocol'
import { List, Network, Undo2, Redo2 } from 'lucide-react'
import type { CSSProperties, RefObject } from 'react'
import { Tooltip } from '../../../components/overlay/Tooltip'
import type { AppProject } from '../../../config/projectConfig'
import type { ChatConversation } from '../../chat/chatTypes'
import { SettingsSelect } from '../../settings/components/SettingsSelect'
import { WorkflowGraphEditor } from '../WorkflowGraphEditor'
import type { useWorkflowHistory } from '../useWorkflowHistory'
import { workflowText } from '../workflowText'
import { WorkflowColorPicker } from './WorkflowColorPicker'
import { projectWorkflowText } from './projectWorkflowText'
import type { BindingDraft } from './workflowBindingDraft'

interface WorkflowBindingEditorProps {
  activeConversations: readonly ChatConversation[]
  busy: boolean
  draft: BindingDraft
  editorId: string
  editorTab: 'details' | 'structure'
  history: ReturnType<typeof useWorkflowHistory>
  language: string
  onBack: () => void
  onBind: (nodeId: string, conversationId: string | null) => void
  onCommit: () => Promise<void>
  onDraftChange: (draft: BindingDraft) => void
  onEditorTabChange: (tab: 'details' | 'structure') => void
  onManageTemplates: () => void
  onTemplateChange: (recordId: string) => void
  projectMissing: boolean
  projects: readonly AppProject[]
  readyRecords: readonly WorkflowRecord[]
  saveButtonRef: RefObject<HTMLButtonElement | null>
  saveDisabled: boolean
  unavailableColors: readonly string[]
}

/** Presentation only; the page retains editor history, draft ownership, and commit sequencing. */
export function WorkflowBindingEditor({
  activeConversations,
  busy,
  draft,
  editorId,
  editorTab,
  history,
  language,
  onBack: back,
  onBind: bind,
  onCommit: commit,
  onDraftChange: setDraft,
  onEditorTabChange: setEditorTab,
  onManageTemplates,
  onTemplateChange: requestTemplate,
  projectMissing,
  projects,
  readyRecords,
  saveButtonRef,
  saveDisabled,
  unavailableColors
}: WorkflowBindingEditorProps) {
  const graph = history.draft
  const text = workflowText(language)
  const t = projectWorkflowText(language)

  return (
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
                      if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return
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
              disabled={saveDisabled}
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
                      unavailableColors={draft.instance?.enabled === false ? [] : unavailableColors}
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
  )
}
