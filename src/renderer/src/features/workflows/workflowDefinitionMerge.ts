import type { WorkflowDefinition } from '@mycopilot/protocol'

interface RemovedEntity {
  kind: 'member' | 'department'
  id: string
  name: string
}

/** Rebase retained local fields while treating remote removals as final, not new local entities. */
export function mergeWorkflowDefinition(
  baseline: WorkflowDefinition,
  local: WorkflowDefinition,
  incoming: WorkflowDefinition
): { definition: WorkflowDefinition; removedEntities: RemovedEntity[] } {
  const removed = new Map<string, RemovedEntity>()
  const changed = (before: unknown, after: unknown) =>
    JSON.stringify(before) !== JSON.stringify(after)
  const mergeFields = <T extends object>(before: T, edited: T, latest: T): T => {
    const result = { ...latest }
    for (const key of new Set([...Object.keys(before), ...Object.keys(edited)]) as Set<keyof T>) {
      if (changed(before[key], edited[key])) result[key] = edited[key]
    }
    return result
  }
  const mergeItems = <T extends { id: string; name: string }>(
    kind: RemovedEntity['kind'],
    before: T[],
    edited: T[],
    latest: T[]
  ): T[] => {
    const oldById = new Map(before.map((item) => [item.id, item]))
    const localById = new Map(edited.map((item) => [item.id, item]))
    const latestById = new Map(latest.map((item) => [item.id, item]))
    const result = latest.flatMap((item) => {
      const old = oldById.get(item.id)
      const edit = localById.get(item.id)
      if (old && !edit) return []
      return [old && edit ? mergeFields(old, edit, item) : item]
    })
    for (const item of edited) {
      if (latestById.has(item.id)) continue
      const old = oldById.get(item.id)
      if (!old) result.push(item)
      else if (changed(old, item))
        removed.set(`${kind}:${item.id}`, { kind, id: item.id, name: item.name })
    }
    return result
  }
  const definition: WorkflowDefinition = {
    ...mergeFields(baseline, local, incoming),
    nodes: mergeItems('member', baseline.nodes, local.nodes, incoming.nodes),
    departments: mergeItems(
      'department',
      baseline.departments ?? [],
      local.departments ?? [],
      incoming.departments ?? []
    ),
    viewport: local.viewport
  }
  const survivingDepartments = new Set(definition.departments?.map((department) => department.id))
  const priorDepartments = new Map(
    [...(baseline.departments ?? []), ...(local.departments ?? [])].map((department) => [
      department.id,
      department
    ])
  )
  // Local additions or moves can still point into a department deleted remotely. Keep those
  // entities, promote them to the nearest surviving ancestor, and report the removed scope.
  const survivingParent = (id: string | null): string | null => {
    const visited = new Set<string>()
    while (id && !survivingDepartments.has(id)) {
      if (visited.has(id)) return null
      visited.add(id)
      const previous = priorDepartments.get(id)
      if (previous) removed.set(`department:${id}`, { kind: 'department', id, name: previous.name })
      id = previous?.parentId ?? null
    }
    return id
  }
  definition.departments = definition.departments?.map((department) => ({
    ...department,
    parentId: survivingParent(department.parentId)
  }))
  const localNodes = new Map(local.nodes.map((node) => [node.id, node]))
  definition.nodes = definition.nodes.map((node) => {
    const departmentId = node.departmentId ? survivingParent(node.departmentId) : node.departmentId
    if (node.managementRole === 'department_admin' && !departmentId) {
      // A remote deletion may have already moved the member to root while a local role edit
      // still targets its former department. Reconcile the role as well as the reference.
      const previousDepartmentId = localNodes.get(node.id)?.departmentId
      if (previousDepartmentId) survivingParent(previousDepartmentId)
      return { ...node, departmentId, managementRole: 'member' as const }
    }
    return node.departmentId === departmentId ? node : { ...node, departmentId }
  })
  return { definition, removedEntities: [...removed.values()] }
}
