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
  return (
    <WorkflowSettingsSection
      onNavigateSettingsRoot={onNavigateSettingsRoot}
      onEditorModeChange={onEditorModeChange}
      onDirtyChange={onDirtyChange}
      onSavingChange={onSavingChange}
    />
  )
}
