// Renderer UI.
import { createContext, useContext, useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import {
  deleteStoredProject,
  loadProjects,
  saveProject as saveStoredProject,
  selectProjectDirectory as selectStoredProjectDirectory,
  showStoredProjectInFolder,
} from "../features/storage/storageClient";
import { createProjectId, projectConfig } from "./projectConfig";
import type { AppProject } from "./projectConfig";

interface ProjectSettingsContextValue {
  addProject: (name: string, path?: string) => AppProject;
  deleteProject: (projectId: string) => void;
  hasLoadedProjects: boolean;
  projects: AppProject[];
  renameProject: (projectId: string, name: string) => void;
  selectProjectDirectory: () => Promise<AppProject | null>;
  showProjectInFolder: (projectId: string) => Promise<void>;
  togglePinProject: (projectId: string) => void;
}

const ProjectSettingsContext = createContext<ProjectSettingsContextValue | null>(null);

export function ProjectSettingsProvider({ children }: { children: ReactNode }) {
  const [projects, setProjects] = useState<AppProject[]>(projectConfig.initialProjects);
  const [hasLoadedProjects, setHasLoadedProjects] = useState(false);

  useEffect(() => {
    let isCancelled = false;

    void loadProjects()
      .then((storedProjects) => {
        if (!isCancelled) {
          setProjects(storedProjects);
        }
      })
      .catch((error) => {
        console.error("Failed to load projects from SQLite", error);
      })
      .finally(() => {
        if (!isCancelled) {
          setHasLoadedProjects(true);
        }
      });

    return () => {
      isCancelled = true;
    };
  }, []);

  const value = useMemo<ProjectSettingsContextValue>(
    () => ({
      addProject: (name, path) => {
        const normalizedName = name.trim();
        const existingProject = projects.find((project) => project.name === normalizedName);
        if (existingProject) {
          if (path && existingProject.path !== path) {
            const updatedProject = { ...existingProject, path };
            setProjects((currentProjects) =>
              currentProjects.map((project) =>
                project.id === existingProject.id ? updatedProject : project,
              ),
            );
            void saveStoredProject(updatedProject).catch((error) => {
              console.error("Failed to save project to SQLite", error);
            });
            return updatedProject;
          }

          return existingProject;
        }

        const project: AppProject = {
          id: createProjectId(normalizedName),
          name: normalizedName,
          path,
          createdAt: Date.now(),
          pinnedAt: null,
        };

        setProjects((currentProjects) => [...currentProjects, project]);
        void saveStoredProject(project).catch((error) => {
          console.error("Failed to save project to SQLite", error);
        });
        return project;
      },
      deleteProject: (projectId) => {
        setProjects((currentProjects) => currentProjects.filter((project) => project.id !== projectId));
        void deleteStoredProject(projectId).catch((error) => {
          console.error("Failed to delete project from SQLite", error);
        });
      },
      hasLoadedProjects,
      projects,
      renameProject: (projectId, name) => {
        const normalizedName = name.trim();
        if (!normalizedName) return;

        setProjects((currentProjects) =>
          currentProjects.map((project) => {
            if (project.id !== projectId) return project;

            const updatedProject = {
              ...project,
              name: normalizedName,
            };

            void saveStoredProject(updatedProject).catch((error) => {
              console.error("Failed to rename project in SQLite", error);
            });

            return updatedProject;
          }),
        );
      },
      selectProjectDirectory: async () => {
        const project = await selectStoredProjectDirectory();
        if (!project) return null;

        setProjects((currentProjects) => {
          const existingIndex = currentProjects.findIndex((currentProject) => currentProject.id === project.id);
          if (existingIndex >= 0) {
            return currentProjects.map((currentProject) =>
              currentProject.id === project.id ? project : currentProject,
            );
          }

          return [...currentProjects.filter((currentProject) => currentProject.path !== project.path), project];
        });

        return project;
      },
      showProjectInFolder: async (projectId) => {
        await showStoredProjectInFolder(projectId);
      },
      togglePinProject: (projectId) => {
        const now = Date.now();

        setProjects((currentProjects) =>
          currentProjects.map((project) => {
            if (project.id !== projectId) return project;

            const updatedProject = {
              ...project,
              pinnedAt: project.pinnedAt ? null : now,
            };

            void saveStoredProject(updatedProject).catch((error) => {
              console.error("Failed to save project pin state to SQLite", error);
            });

            return updatedProject;
          }),
        );
      },
    }),
    [hasLoadedProjects, projects],
  );

  return <ProjectSettingsContext.Provider value={value}>{children}</ProjectSettingsContext.Provider>;
}

export function useProjectSettings() {
  const context = useContext(ProjectSettingsContext);

  if (!context) {
    throw new Error("useProjectSettings must be used within ProjectSettingsProvider");
  }

  return context;
}
