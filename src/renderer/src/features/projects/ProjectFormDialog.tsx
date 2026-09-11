import { Folder, Plus, X } from 'lucide-react'
import { useEffect, useId, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import type { StorageProjectFolderPick, StorageProjectValidationCode } from '@mycopilot/protocol'
import { MAX_PROJECT_FOLDERS } from '@mycopilot/protocol'
import type { TranslationKey } from '../../config/frontendTranslations'
import type { AppProjectFolderRole } from '../../config/projectConfig'
import { projectFolderDisplayName } from '../../config/projectConfig'
import { ProjectValidationError } from './projectValidationError'
import '../../components/dialog/ConfirmationDialog.css'
import './ProjectFormDialog.css'

export interface ProjectFormFolder {
  /** Stored folder id when editing; absent for folders added in this dialog session. */
  id?: string
  path: string
  role: AppProjectFolderRole
}

export interface ProjectFormValues {
  name: string
  folders: ProjectFormFolder[]
}

interface ProjectFormDialogProps {
  initialFolders: ProjectFormFolder[]
  initialName: string
  mode: 'create' | 'edit'
  onCancel: () => void
  onPickFolder: () => Promise<StorageProjectFolderPick | null>
  /** Removes the project itself; only offered while editing. */
  onRemoveProject?: () => void
  onSubmit: (values: ProjectFormValues) => Promise<void>
  t: (key: TranslationKey) => string
}

const VALIDATION_MESSAGE_KEYS: Record<StorageProjectValidationCode, TranslationKey> = {
  name_required: 'project.formError.nameRequired',
  folders_required: 'project.formError.foldersRequired',
  primary_required: 'project.formError.primaryRequired',
  too_many_folders: 'project.formError.tooManyFolders',
  folder_missing: 'project.formError.folderMissing',
  folder_duplicate: 'project.formError.folderDuplicate',
  folder_nested: 'project.formError.folderNested',
  project_missing: 'project.formError.projectMissing'
}

function normalizePathForComparison(path: string): string {
  return path.trim().replace(/[\\/]+$/, '')
}

/** Keeps exactly one primary folder: the first folder inherits the role when it goes missing. */
function withSinglePrimary(folders: ProjectFormFolder[]): ProjectFormFolder[] {
  if (folders.length === 0) return folders
  const primaryIndex = folders.findIndex((folder) => folder.role === 'primary')
  const targetIndex = primaryIndex >= 0 ? primaryIndex : 0
  return folders.map((folder, index) => ({
    ...folder,
    role: index === targetIndex ? 'primary' : 'auxiliary'
  }))
}

export function ProjectFormDialog({
  initialFolders,
  initialName,
  mode,
  onCancel,
  onPickFolder,
  onRemoveProject,
  onSubmit,
  t
}: ProjectFormDialogProps) {
  const titleId = useId()
  const nameInputId = useId()
  const foldersHeadingId = useId()
  const errorId = useId()
  const [name, setName] = useState(initialName)
  const [folders, setFolders] = useState<ProjectFormFolder[]>(() =>
    withSinglePrimary(initialFolders)
  )
  const [error, setError] = useState<string | null>(null)
  const [isBusy, setIsBusy] = useState(false)
  const busyRef = useRef(false)
  const nameInputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    const frameId = window.requestAnimationFrame(() => nameInputRef.current?.focus())
    return () => window.cancelAnimationFrame(frameId)
  }, [])

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      event.preventDefault()
      event.stopImmediatePropagation()
      if (!busyRef.current) onCancel()
    }
    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [onCancel])

  const runExclusive = async (operation: () => Promise<void>) => {
    if (busyRef.current) return
    busyRef.current = true
    setIsBusy(true)
    try {
      await operation()
    } finally {
      busyRef.current = false
      setIsBusy(false)
    }
  }

  const addFolder = () =>
    runExclusive(async () => {
      const pick = await onPickFolder()
      if (!pick) return
      const pickedPath = normalizePathForComparison(pick.path)
      if (folders.some((folder) => normalizePathForComparison(folder.path) === pickedPath)) {
        setError(t('project.formError.folderDuplicate'))
        return
      }
      if (folders.length >= MAX_PROJECT_FOLDERS) {
        setError(t('project.formError.tooManyFolders'))
        return
      }
      setError(null)
      setFolders((current) =>
        withSinglePrimary([
          ...current,
          { path: pick.path, role: current.length === 0 ? 'primary' : 'auxiliary' }
        ])
      )
      if (!name.trim()) setName(pick.name || projectFolderDisplayName(pick.path))
    })

  const removeFolder = (index: number) => {
    setError(null)
    setFolders((current) => withSinglePrimary(current.filter((_, position) => position !== index)))
  }

  const makePrimary = (index: number) => {
    setError(null)
    setFolders((current) =>
      current.map((folder, position) => ({
        ...folder,
        role: position === index ? 'primary' : 'auxiliary'
      }))
    )
  }

  const submit = () =>
    runExclusive(async () => {
      const trimmedName = name.trim()
      if (!trimmedName) {
        setError(t('project.formError.nameRequired'))
        nameInputRef.current?.focus()
        return
      }
      if (folders.length === 0) {
        setError(t('project.formError.foldersRequired'))
        return
      }
      setError(null)
      try {
        await onSubmit({ name: trimmedName, folders: withSinglePrimary(folders) })
      } catch (submitError) {
        if (submitError instanceof ProjectValidationError) {
          const message = t(VALIDATION_MESSAGE_KEYS[submitError.data.code])
          setError(submitError.data.path ? `${message} (${submitError.data.path})` : message)
          return
        }
        console.error('Project save failed', submitError)
        setError(t('project.formError.saveFailed'))
      }
    })

  const canSubmit = !isBusy && name.trim().length > 0 && folders.length > 0

  return createPortal(
    <div
      className="app-confirm-dialog__backdrop"
      role="presentation"
      onMouseDown={(event) => {
        if (!isBusy && event.currentTarget === event.target) onCancel()
      }}
    >
      <form
        className="app-confirm-dialog__card project-form-dialog"
        role="dialog"
        aria-modal="true"
        aria-busy={isBusy || undefined}
        aria-labelledby={titleId}
        onSubmit={(event) => {
          event.preventDefault()
          if (canSubmit) void submit()
        }}
      >
        <button
          className="app-confirm-dialog__close"
          type="button"
          aria-label={t('project.cancel')}
          disabled={isBusy}
          onClick={onCancel}
        >
          <X aria-hidden="true" />
        </button>
        <h2 id={titleId}>
          {mode === 'create' ? t('project.createTitle') : t('project.editTitle')}
        </h2>

        <label className="project-form-dialog__label" htmlFor={nameInputId}>
          {t('project.nameLabel')}
        </label>
        <input
          ref={nameInputRef}
          id={nameInputId}
          className="app-confirm-dialog__input project-form-dialog__name"
          placeholder={t('project.namePlaceholder')}
          value={name}
          disabled={isBusy}
          onChange={(event) => {
            setError(null)
            setName(event.target.value)
          }}
        />

        <div className="project-form-dialog__folders-header">
          <span className="project-form-dialog__label" id={foldersHeadingId}>
            {t('project.foldersLabel')}
          </span>
          <p className="project-form-dialog__hint">{t('project.foldersDescription')}</p>
        </div>
        <ul className="project-form-dialog__folders" aria-labelledby={foldersHeadingId}>
          {folders.length === 0 && (
            <li className="project-form-dialog__empty">{t('project.noFolders')}</li>
          )}
          {folders.map((folder, index) => {
            const displayName = projectFolderDisplayName(folder.path)
            return (
              <li className="project-form-dialog__folder" key={folder.id ?? folder.path}>
                <Folder aria-hidden="true" />
                <div className="project-form-dialog__folder-text">
                  <span className="project-form-dialog__folder-name">{displayName}</span>
                  <span className="project-form-dialog__folder-path" title={folder.path}>
                    {folder.path}
                  </span>
                </div>
                {folder.role === 'primary' ? (
                  <span className="project-form-dialog__badge">{t('project.primaryFolder')}</span>
                ) : (
                  <button
                    className="project-form-dialog__folder-action"
                    type="button"
                    disabled={isBusy}
                    aria-label={`${t('project.setPrimaryFolder')} ${displayName}`}
                    onClick={() => makePrimary(index)}
                  >
                    {t('project.setPrimaryFolder')}
                  </button>
                )}
                <button
                  className="project-form-dialog__folder-remove"
                  type="button"
                  disabled={isBusy}
                  aria-label={`${t('project.removeFolder')} ${displayName}`}
                  onClick={() => removeFolder(index)}
                >
                  <X aria-hidden="true" />
                </button>
              </li>
            )
          })}
        </ul>
        <button
          className="project-form-dialog__add-folder"
          type="button"
          disabled={isBusy || folders.length >= MAX_PROJECT_FOLDERS}
          onClick={() => void addFolder()}
        >
          <Plus aria-hidden="true" />
          <span>{t('project.addFolder')}</span>
        </button>

        {error && (
          <p className="project-form-dialog__error" id={errorId} role="alert">
            {error}
          </p>
        )}

        <div className="app-confirm-dialog__actions project-form-dialog__actions">
          {mode === 'edit' && onRemoveProject && (
            <button
              className="app-confirm-dialog__button project-form-dialog__remove-project"
              type="button"
              disabled={isBusy}
              onClick={onRemoveProject}
            >
              {t('project.removeLocalProject')}
            </button>
          )}
          <span className="project-form-dialog__actions-spacer" aria-hidden="true" />
          <button
            className="app-confirm-dialog__button app-confirm-dialog__button--cancel"
            type="button"
            disabled={isBusy}
            onClick={onCancel}
          >
            {t('project.cancel')}
          </button>
          <button
            className="app-confirm-dialog__button app-confirm-dialog__button--primary"
            type="submit"
            disabled={!canSubmit}
          >
            {mode === 'create' ? t('project.create') : t('project.save')}
          </button>
        </div>
      </form>
    </div>,
    document.body
  )
}
