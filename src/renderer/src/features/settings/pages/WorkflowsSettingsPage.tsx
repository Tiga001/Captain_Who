import { WorkflowSettingsSection } from '../../workflows/WorkflowSettingsSection'

export function WorkflowsSettingsPage({
  onEditorModeChange,
  onDirtyChange,
  onSavingChange
}: {
  onEditorModeChange?: (editing: boolean) => void
  onDirtyChange?: (dirty: boolean) => void
  onSavingChange?: (saving: boolean) => void
}) {
  return (
    <WorkflowSettingsSection
      onEditorModeChange={onEditorModeChange}
      onDirtyChange={onDirtyChange}
      onSavingChange={onSavingChange}
    />
  )
}
