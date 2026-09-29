import { Folder, X } from 'lucide-react'
import type { AgentFolderReference } from '@mycopilot/protocol'
import { hostClient } from '../../../host/hostClient'
import './AttachmentCards.css'

interface ComposerFolderReferencesProps {
  folders: readonly AgentFolderReference[]
  label: string
  removeLabel: string
  onRemove?: (id: string) => void
  canOpen?: boolean
}

/** Compact, non-uploading folder grants shown alongside regular attachments. */
export function ComposerFolderReferences({
  folders,
  label,
  removeLabel,
  onRemove,
  canOpen = true
}: ComposerFolderReferencesProps) {
  if (folders.length === 0) return null
  return (
    <div className="composer-folder-references" aria-label={label}>
      {folders.map((folder) => (
        <div
          className="composer-folder-reference"
          data-folder-reference-id={folder.id}
          key={folder.id}
          title={folder.rootPath ?? folder.name}
        >
          <button
            type="button"
            className="composer-folder-reference__open"
            disabled={!canOpen || !folder.rootPath || folder.status === 'unavailable'}
            onClick={() =>
              void hostClient.attachments.openFolder({ folder }).catch(() => undefined)
            }
          >
            <Folder aria-hidden="true" className="composer-folder-reference__icon" />
            <span className="composer-folder-reference__name">{folder.name}</span>
          </button>
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
