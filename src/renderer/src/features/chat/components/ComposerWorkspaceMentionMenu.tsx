import type { WorkspaceMentionSearchEntry } from '@mycopilot/protocol'
import { Folder } from 'lucide-react'
import { WorkspaceFileTypeIcon } from '../../../components/files/WorkspaceFileTypeIcon'

interface ComposerWorkspaceMentionMenuProps {
  mentionEntries: readonly WorkspaceMentionSearchEntry[]
  mentionIndex: number
  setMentionIndex: (index: number) => void
  selectWorkspaceMention: (entry: WorkspaceMentionSearchEntry) => void
}

export function ComposerWorkspaceMentionMenu({
  mentionEntries,
  mentionIndex,
  setMentionIndex,
  selectWorkspaceMention
}: ComposerWorkspaceMentionMenuProps) {
  return mentionEntries.length === 0 ? (
    <div className="composer-commands" role="listbox" aria-label="Workspace files">
      <div className="composer-commands__list">
        <p className="composer-commands__empty">未找到匹配项</p>
      </div>
    </div>
  ) : (
    <div className="composer-commands" role="listbox" aria-label="Workspace files">
      <div className="composer-commands__list">
        {mentionEntries.map((entry, index) => (
          <button
            key={`${entry.folderId}:${entry.path}`}
            type="button"
            role="option"
            aria-selected={index === mentionIndex}
            onMouseDown={(event) => event.preventDefault()}
            onPointerMove={() => setMentionIndex(index)}
            onClick={() => selectWorkspaceMention(entry)}
          >
            {entry.kind === 'directory' ? (
              <Folder aria-hidden="true" />
            ) : (
              <WorkspaceFileTypeIcon path={entry.path} />
            )}
            <span className="composer-commands__label">{entry.displayName}</span>
            <span className="composer-commands__description" title={entry.displayPath}>
              {entry.displayPath}
            </span>
          </button>
        ))}
      </div>
    </div>
  )
}
