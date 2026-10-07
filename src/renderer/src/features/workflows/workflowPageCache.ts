import type { WorkflowRequest, WorkflowResponse } from '@mycopilot/protocol'

const MAX_ENTRIES = 33 // One catalog plus the most recently opened organizations.
const FRESH_MS = 15_000
type Entry = { response: WorkflowResponse; expiresAt: number }
const snapshots = new Map<string, Entry>()
const pending = new Map<string, { generation: number; promise: Promise<WorkflowResponse> }>()
let generation = 0
const keyFor = (instanceId: string | null) =>
  instanceId === null ? 'catalog' : `instance:${instanceId}`

/** Memory-only and bounded; navigation/draft remounts do not throw away a readable board. */
export function getCachedWorkflowPage(instanceId: string | null): WorkflowResponse | null {
  const key = keyFor(instanceId)
  const entry = snapshots.get(key)
  if (entry && entry.expiresAt > Date.now()) {
    snapshots.delete(key)
    snapshots.set(key, entry)
    return entry.response
  }
  snapshots.delete(key)
  if (instanceId === null) return null
  const catalog = getCachedWorkflowPage(null)
  const instance = catalog?.instances?.find((item) => item.id === instanceId)
  return instance ? { records: [], issues: [], instances: [instance] } : null
}

export function invalidateWorkflowPages(): void {
  generation += 1
  snapshots.clear()
}

/** A newer notification can arrive between a resolved read and its React continuation. */
export function isCurrentWorkflowPage(
  instanceId: string | null,
  response: WorkflowResponse
): boolean {
  return snapshots.get(keyFor(instanceId))?.response === response
}

/** All page reads share a flight, including a focus refresh during initial loading. */
export function readWorkflowPage(
  instanceId: string | null,
  read: (request: WorkflowRequest) => Promise<WorkflowResponse>,
  force = false
): Promise<WorkflowResponse> {
  const key = keyFor(instanceId)
  const running = pending.get(key)
  if (running?.generation === generation) return running.promise
  const cached = force ? null : getCachedWorkflowPage(instanceId)
  if (cached) return Promise.resolve(cached)
  const requestedGeneration = generation
  const promise = read(
    instanceId === null ? { operation: 'list' } : { operation: 'getInstance', instanceId }
  )
    .then(
      (response) => {
        // A mutation invalidated this flight. Never repaint a deleted/older organization;
        // join a fresh read before either the initial loader or background refresh resumes.
        if (requestedGeneration !== generation) return readWorkflowPage(instanceId, read, true)
        snapshots.delete(key)
        snapshots.set(key, { response, expiresAt: Date.now() + FRESH_MS })
        while (snapshots.size > MAX_ENTRIES) snapshots.delete(snapshots.keys().next().value!)
        return response
      },
      (error: unknown) => {
        if (requestedGeneration !== generation) return readWorkflowPage(instanceId, read, true)
        throw error
      }
    )
    .finally(() => {
      if (pending.get(key)?.promise === promise) pending.delete(key)
    })
  pending.set(key, { generation: requestedGeneration, promise })
  return promise
}
