import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react'
import type { ReactNode } from 'react'
import { useAppStartupStage } from '../features/startup/AppStartupContext'
import {
  ProjectFormDialog,
  type ProjectFormFolder,
  type ProjectFormValues
} from '../features/projects/ProjectFormDialog'
import {
  createProject as createStoredProject,
  deleteStoredProject,
  loadProjects,
  pickProjectFolder,
  revealStoredProjectFile,
  saveProject as saveStoredProject,
  showStoredProjectInFolder,
  updateProject as updateStoredProject
} from '../features/storage/storageClient'
import { useFrontendConfig } from './FrontendConfigProvider'
import type { AppProject } from './projectConfig'

/** Outcome of the edit dialog; `remove-requested` lets the caller run its removal confirmation. */
export type ProjectEditDialogResult = 'saved' | 'remove-requested' | 'cancelled'

interface ProjectSettingsContextValue {
  deleteProject: (projectId: string) => Promise<void>
  /** Opens the create dialog and resolves with the stored project, or null when dismissed. */
  openCreateProjectDialog: () => Promise<AppProject | null>
  /** Opens the edit dialog for one project (name, folders, primary folder, removal entry). */
  openEditProjectDialog: (projectId: string) => Promise<ProjectEditDialogResult>
  projects: AppProject[]
  showProjectInFolder: (projectId: string) => Promise<void>
  /** Reveals one of the project's folders in the OS file manager. */
  showProjectFolder: (projectId: string, folderId: string) => Promise<void>
  togglePinProject: (projectId: string) => void
}

type ProjectDialogState =
  | { mode: 'create'; resolve: (project: AppProject | null) => void }
  | { mode: 'edit'; project: AppProject; resolve: (result: ProjectEditDialogResult) => void }

const ProjectSettingsContext = createContext<ProjectSettingsContextValue | null>(null)

export function ProjectSettingsProvider({ children }: { children: ReactNode }) {
  const { t } = useFrontendConfig()
  const [projects, setProjects] = useState<AppProject[]>([])
  const [dialog, setDialog] = useState<ProjectDialogState | null>(null)
  const dialogRef = useRef<ProjectDialogState | null>(null)
  const {
    attempt: startupAttempt,
    markFailed: markStartupFailed,
    markPending: markStartupPending,
    markReady: markStartupReady
  } = useAppStartupStage('projects')

  useEffect(() => {
    let isCancelled = false
    markStartupPending()

    void loadProjects()
      .then((storedProjects) => {
        if (!isCancelled) {
          setProjects(storedProjects)
          markStartupReady()
        }
      })
      .catch((error) => {
        if (!isCancelled) {
          markStartupFailed(error)
          console.error('Failed to load projects from SQLite', error)
        }
      })

    return () => {
      isCancelled = true
    }
  }, [markStartupFailed, markStartupPending, markStartupReady, startupAttempt])

  const openDialog = useCallback((next: ProjectDialogState) => {
    // A dialog that is still open when another one is requested counts as dismissed.
    const previous = dialogRef.current
    if (previous?.mode === 'create') previous.resolve(null)
    if (previous?.mode === 'edit') previous.resolve('cancelled')
    dialogRef.current = next
    setDialog(next)
  }, [])

  const closeDialog = useCallback(() => {
    dialogRef.current = null
    setDialog(null)
  }, [])

  const upsertProject = useCallback((project: AppProject) => {
    setProjects((currentProjects) => {
      const exists = currentProjects.some((currentProject) => currentProject.id === project.id)
      return exists
        ? currentProjects.map((currentProject) =>
            currentProject.id === project.id ? project : currentProject
          )
        : [...currentProjects, project]
    })
  }, [])

  const value = useMemo<ProjectSettingsContextValue>(
    () => ({
      deleteProject: async (projectId) => {
        await deleteStoredProject(projectId)
        setProjects((currentProjects) =>
          currentProjects.filter((project) => project.id !== projectId)
        )
      },
      openCreateProjectDialog: () =>
        new Promise<AppProject | null>((resolve) => {
          openDialog({ mode: 'create', resolve })
        }),
      openEditProjectDialog: (projectId) =>
        new Promise<ProjectEditDialogResult>((resolve) => {
          const project = projects.find((currentProject) => currentProject.id === projectId)
          if (!project) {
            resolve('cancelled')
            return
          }
          openDialog({ mode: 'edit', project, resolve })
        }),
      projects,
      showProjectInFolder: async (projectId) => {
        await showStoredProjectInFolder(projectId)
      },
      showProjectFolder: async (projectId, folderId) => {
        const folder = projects
          .find((project) => project.id === projectId)
          ?.folders.find((candidate) => candidate.id === folderId)
        if (!folder) return
        await revealStoredProjectFile(projectId, folder.path)
      },
      togglePinProject: (projectId) => {
        const now = Date.now()

        setProjects((currentProjects) =>
          currentProjects.map((project) => {
            if (project.id !== projectId) return project

            const updatedProject = {
              ...project,
              pinnedAt: project.pinnedAt ? null : now
            }

            void saveStoredProject(updatedProject).catch((error) => {
              console.error('Failed to save project pin state to SQLite', error)
            })

            return updatedProject
          })
        )
      }
    }),
    [openDialog, projects]
  )

  const submitDialog = async (values: ProjectFormValues) => {
    const current = dialogRef.current
    if (!current) return
    const folders = values.folders.map((folder) => ({
      ...(folder.id ? { id: folder.id } : {}),
      path: folder.path,
      role: folder.role
    }))
    if (current.mode === 'create') {
      const created = await createStoredProject({ name: values.name, folders })
      upsertProject(created)
      closeDialog()
      current.resolve(created)
      return
    }
    const updated = await updateStoredProject({
      projectId: current.project.id,
      name: values.name,
      folders
    })
    upsertProject(updated)
    closeDialog()
    current.resolve('saved')
  }

  const cancelDialog = () => {
    const current = dialogRef.current
    closeDialog()
    if (current?.mode === 'create') current.resolve(null)
    if (current?.mode === 'edit') current.resolve('cancelled')
  }

  const requestRemoveFromDialog = () => {
    const current = dialogRef.current
    closeDialog()
    if (current?.mode === 'edit') current.resolve('remove-requested')
  }

  const initialFolders: ProjectFormFolder[] =
    dialog?.mode === 'edit'
      ? dialog.project.folders.map((folder) => ({
          id: folder.id,
          path: folder.path,
          role: folder.role
        }))
      : []

  return (
    <ProjectSettingsContext.Provider value={value}>
      {children}
      {dialog && (
        <ProjectFormDialog
          key={dialog.mode === 'edit' ? `edit:${dialog.project.id}` : 'create'}
          initialFolders={initialFolders}
          initialName={dialog.mode === 'edit' ? dialog.project.name : ''}
          mode={dialog.mode}
          onCancel={cancelDialog}
          onPickFolder={pickProjectFolder}
          onRemoveProject={dialog.mode === 'edit' ? requestRemoveFromDialog : undefined}
          onSubmit={submitDialog}
          t={t}
        />
      )}
    </ProjectSettingsContext.Provider>
  )
}

export function useProjectSettings() {
  const context = useContext(ProjectSettingsContext)

  if (!context) {
    throw new Error('useProjectSettings must be used within ProjectSettingsProvider')
  }

  return context
}
