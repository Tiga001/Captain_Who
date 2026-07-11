import { createContext, useContext, useEffect, useMemo, useState } from 'react'
import type { ReactNode } from 'react'
import {
  deleteStoredProject,
  loadProjects,
  saveProject as saveStoredProject,
  selectProjectDirectory as selectStoredProjectDirectory,
  showStoredProjectInFolder
} from '../features/storage/storageClient'
import type { AppProject } from './projectConfig'

interface ProjectSettingsContextValue {
  deleteProject: (projectId: string) => Promise<void>
  projects: AppProject[]
  renameProject: (projectId: string, name: string) => void
  selectProjectDirectory: () => Promise<AppProject | null>
  showProjectInFolder: (projectId: string) => Promise<void>
  togglePinProject: (projectId: string) => void
}

const ProjectSettingsContext = createContext<ProjectSettingsContextValue | null>(null)

export function ProjectSettingsProvider({ children }: { children: ReactNode }) {
  const [projects, setProjects] = useState<AppProject[]>([])

  useEffect(() => {
    let isCancelled = false

    void loadProjects()
      .then((storedProjects) => {
        if (!isCancelled) {
          setProjects(storedProjects)
        }
      })
      .catch((error) => {
        console.error('Failed to load projects from SQLite', error)
      })

    return () => {
      isCancelled = true
    }
  }, [])

  const value = useMemo<ProjectSettingsContextValue>(
    () => ({
      deleteProject: async (projectId) => {
        await deleteStoredProject(projectId)
        setProjects((currentProjects) =>
          currentProjects.filter((project) => project.id !== projectId)
        )
      },
      projects,
      renameProject: (projectId, name) => {
        const normalizedName = name.trim()
        if (!normalizedName) return

        setProjects((currentProjects) =>
          currentProjects.map((project) => {
            if (project.id !== projectId) return project

            const updatedProject = {
              ...project,
              name: normalizedName
            }

            void saveStoredProject(updatedProject).catch((error) => {
              console.error('Failed to rename project in SQLite', error)
            })

            return updatedProject
          })
        )
      },
      selectProjectDirectory: async () => {
        const selectedProject = await selectStoredProjectDirectory()
        if (!selectedProject) return null

        const project =
          projects.find(
            (currentProject) => currentProject.path && currentProject.path === selectedProject.path
          ) ?? selectedProject

        setProjects((currentProjects) => {
          const existingIndex = currentProjects.findIndex(
            (currentProject) => currentProject.id === project.id
          )
          if (existingIndex >= 0) {
            return currentProjects.map((currentProject) =>
              currentProject.id === project.id ? project : currentProject
            )
          }

          return [
            ...currentProjects.filter((currentProject) => currentProject.path !== project.path),
            project
          ]
        })
        void saveStoredProject(project).catch((error) => {
          console.error('Failed to save selected project to SQLite', error)
        })

        return project
      },
      showProjectInFolder: async (projectId) => {
        await showStoredProjectInFolder(projectId)
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
    [projects]
  )

  return <ProjectSettingsContext.Provider value={value}>{children}</ProjectSettingsContext.Provider>
}

export function useProjectSettings() {
  const context = useContext(ProjectSettingsContext)

  if (!context) {
    throw new Error('useProjectSettings must be used within ProjectSettingsProvider')
  }

  return context
}
