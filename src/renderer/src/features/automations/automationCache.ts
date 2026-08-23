import type { AutomationTask } from './automationTypes'

type CacheListener = () => void

let tasks = new Map<string, AutomationTask>()
const tombstones = new Map<string, number>()
const listeners = new Set<CacheListener>()

function latestActivity(task: AutomationTask): number {
  return Math.max(
    task.updatedAt,
    task.latestRun?.updatedAt ?? 0,
    task.attention?.createdAt ?? 0,
    task.attention?.readAt ?? 0
  )
}

function emit(): void {
  for (const listener of listeners) listener()
}

export function getCachedAutomationTask(automationId: string): AutomationTask | null {
  return tasks.get(automationId) ?? null
}

export function getAutomationCacheSnapshot(): ReadonlyMap<string, AutomationTask> {
  return tasks
}

export function cacheAutomationTask(task: AutomationTask): boolean {
  const tombstoneRevision = tombstones.get(task.automationId)
  if (tombstoneRevision !== undefined && task.revision <= tombstoneRevision) return false
  const current = tasks.get(task.automationId)
  if (
    current &&
    (current.revision > task.revision ||
      (current.revision === task.revision && latestActivity(current) >= latestActivity(task)))
  ) {
    return false
  }
  if (current === task) return false
  tasks = new Map(tasks).set(task.automationId, task)
  tombstones.delete(task.automationId)
  emit()
  return true
}

export function cacheAutomationTasks(nextTasks: readonly AutomationTask[]): void {
  let changed = false
  const next = new Map(tasks)
  for (const task of nextTasks) {
    const tombstoneRevision = tombstones.get(task.automationId)
    if (tombstoneRevision !== undefined && task.revision <= tombstoneRevision) continue
    const current = next.get(task.automationId)
    if (
      current &&
      (current.revision > task.revision ||
        (current.revision === task.revision && latestActivity(current) >= latestActivity(task)))
    ) {
      continue
    }
    if (current !== task) {
      next.set(task.automationId, task)
      tombstones.delete(task.automationId)
      changed = true
    }
  }
  if (!changed) return
  tasks = next
  emit()
}

export function removeCachedAutomationTask(
  automationId: string,
  revision = Number.MAX_SAFE_INTEGER
): void {
  tombstones.set(automationId, Math.max(tombstones.get(automationId) ?? 0, revision))
  if (!tasks.has(automationId)) return
  tasks = new Map(tasks)
  tasks.delete(automationId)
  emit()
}

export function subscribeAutomationCache(listener: CacheListener): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

/** Test-only reset kept explicit so production never clears authoritative UI state implicitly. */
export function resetAutomationCacheForTests(): void {
  tasks = new Map()
  tombstones.clear()
  emit()
}
