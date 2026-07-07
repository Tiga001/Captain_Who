// Renderer UI.
export interface AppProject {
  id: string;
  name: string;
  path?: string;
  createdAt: number;
  pinnedAt?: number | null;
}

export const projectConfig = {
  initialProjects: [] satisfies AppProject[],
} as const;

export function createProjectId(name: string) {
  const normalizedName = name.toLowerCase().replace(/[^a-z0-9\u4e00-\u9fa5]+/g, "-");
  return `project-${normalizedName || "local"}-${Date.now()}`;
}
