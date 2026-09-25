import { useCallback, useEffect, useRef, useState } from 'react'
import type { AgentTemplate } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { listAgentTemplates } from '../../agentCollaboration/collaborationClient'
import { WorkflowSettingsSection } from '../../workflows/WorkflowSettingsSection'

export function WorkflowsSettingsPage({
  onNavigateSettingsRoot,
  onEditorModeChange,
  onDirtyChange,
  onSavingChange
}: {
  onNavigateSettingsRoot?: () => void
  onEditorModeChange?: (editing: boolean) => void
  onDirtyChange?: (dirty: boolean) => void
  onSavingChange?: (saving: boolean) => void
}) {
  const { t } = useFrontendConfig()
  const [templates, setTemplates] = useState<AgentTemplate[] | null>(null)
  const [loading, setLoading] = useState(true)
  const [failed, setFailed] = useState(false)
  const sequence = useRef(0)
  const load = useCallback(async () => {
    const request = ++sequence.current
    setLoading(true)
    setFailed(false)
    try {
      const result = await listAgentTemplates({ includeDisabled: true })
      if (request !== sequence.current) return
      setTemplates(
        [...result.templates].sort(
          (left, right) =>
            Number(right.enabled) - Number(left.enabled) ||
            left.name.localeCompare(right.name) ||
            left.templateId.localeCompare(right.templateId)
        )
      )
    } catch {
      if (request === sequence.current) setFailed(true)
    } finally {
      if (request === sequence.current) setLoading(false)
    }
  }, [])
  useEffect(() => {
    void load()
    return () => {
      sequence.current += 1
    }
  }, [load])

  return (
    <>
      {loading ? <p role="status">{t('workflows.templatesLoading')}</p> : null}
      {failed ? (
        <div role="alert" className="workflow-error">
          <span>{t('workflows.templatesLoadFailed')}</span>
          <button
            type="button"
            className="workflow-button"
            disabled={loading}
            onClick={() => void load()}
          >
            {t('agentTemplates.retry')}
          </button>
        </div>
      ) : null}
      {templates ? (
        <WorkflowSettingsSection
          templates={templates}
          onNavigateSettingsRoot={onNavigateSettingsRoot}
          onEditorModeChange={onEditorModeChange}
          onDirtyChange={onDirtyChange}
          onSavingChange={onSavingChange}
        />
      ) : null}
    </>
  )
}
