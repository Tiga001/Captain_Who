export interface AppProject {
  id: string
  name: string
  path?: string
  createdAt: number
  pinnedAt?: number | null
}
