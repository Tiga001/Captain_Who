import { Folder, X } from 'lucide-react'
import type { AgentFolderReference } from '@mycopilot/protocol'
import './AttachmentCards.css'

interface ComposerFolderReferencesProps {
  folders: readonly AgentFolderReference[]
  label: string
  removeLabel: string
  onRemove?: (id: string) => void
}

/** Compact, non-uploading folder grants shown alongside regular attachments. */
export function ComposerFolderReferences({
  folders,
  label,
  removeLabel,
  onRemove
}: ComposerFolderReferencesProps) {
  if (folders.length === 0) return null
  return (
    <div className="composer-folder-references" aria-label={label}>
      {folders.map((folder) => (
        <div
          className="composer-folder-reference"
          data-folder-reference-id={folder.id}
          key={folder.id}
          title={folder.name}
        >
          <Folder aria-hidden="true" className="composer-folder-reference__icon" />
          <span className="composer-folder-reference__name">{folder.name}</span>
          {onRemove && (
            <button
              type="button"
              className="composer-folder-reference__remove"
              aria-label={`${removeLabel} ${folder.name}`}
              onClick={() => onRemove(folder.id)}
            >
              <X aria-hidden="true" />
            </button>
          )}
        </div>
      ))}
    </div>
  )
}
