import type { WorkflowDefinition } from '@mycopilot/protocol'
import { availableWorkflowMemberName, workflowNodeSize } from './workflowAuthoring'

export type WorkflowDepartment = NonNullable<WorkflowDefinition['departments']>[number]
type Rectangle = { x: number; y: number; width: number; height: number }

export const MIN_DEPARTMENT_WIDTH = 80
export const MIN_DEPARTMENT_HEIGHT = 64
export const MAX_WORKFLOW_DEPARTMENTS = 64

function contains(outer: Rectangle, inner: Rectangle) {
  return (
    outer.x <= inner.x &&
    outer.y <= inner.y &&
    outer.x + outer.width >= inner.x + inner.width &&
    outer.y + outer.height >= inner.y + inner.height
  )
}

function overlaps(first: Rectangle, second: Rectangle) {
  return (
    first.x < second.x + second.width &&
    first.x + first.width > second.x &&
    first.y < second.y + second.height &&
    first.y + first.height > second.y
  )
}

/** Frames form a tree: siblings cannot overlap, and identical frames are ambiguous. */
export function canPlaceWorkflowDepartments(departments: readonly WorkflowDepartment[]) {
  return departments.every(
    (department, index) =>
      Number.isFinite(department.x) &&
      Math.abs(department.x) <= 100000 &&
      Number.isFinite(department.y) &&
      Math.abs(department.y) <= 100000 &&
      Number.isFinite(department.width) &&
      department.width >= MIN_DEPARTMENT_WIDTH &&
      department.width <= 100000 &&
      Number.isFinite(department.height) &&
      department.height >= MIN_DEPARTMENT_HEIGHT &&
      department.height <= 100000 &&
      departments.slice(index + 1).every((other) => {
        const containsOther = contains(department, other)
        const insideOther = contains(other, department)
        return (
          !(containsOther && insideOther) &&
          (!overlaps(department, other) || containsOther || insideOther)
        )
      })
  )
}

/** Department membership is determined by the complete node rectangle, never its center. */
export function normalizeWorkflowDepartments(graph: WorkflowDefinition): WorkflowDefinition {
  const departments = graph.departments ?? []
  if (!canPlaceWorkflowDepartments(departments)) return graph
  const byArea = [...departments].sort((a, b) => a.width * a.height - b.width * b.height)
  return {
    ...graph,
    departments: departments.map((department) => ({
      ...department,
      parentId:
        byArea.find(
          (candidate) => candidate.id !== department.id && contains(candidate, department)
        )?.id ?? null
    })),
    nodes: graph.nodes.map((node) => {
      const departmentId =
        byArea.find((department) => contains(department, { ...node, ...workflowNodeSize(node) }))
          ?.id ?? null
      return {
        ...node,
        ...(departmentId || node.departmentId !== undefined ? { departmentId } : {}),
        ...(node.managementRole === 'department_admin' && !departmentId
          ? { managementRole: 'member' as const }
          : {})
      }
    })
  }
}

/** Infer nesting before choosing a default name; only sibling names must be distinct. */
export function addWorkflowDepartment(graph: WorkflowDefinition, department: WorkflowDepartment) {
  const normalized = normalizeWorkflowDepartments({
    ...graph,
    departments: [...(graph.departments ?? []), department]
  })
  const created = normalized.departments?.find((item) => item.id === department.id)
  if (!created) return graph
  const name = availableWorkflowMemberName(
    created.name,
    normalized.departments?.filter(
      (item) => item.id !== created.id && item.parentId === created.parentId
    ) ?? []
  )
  return {
    ...normalized,
    departments: normalized.departments?.map((item) =>
      item.id === created.id ? { ...item, name } : item
    )
  }
}

/** Removing a frame preserves its people and child departments. */
export function removeWorkflowDepartment(graph: WorkflowDefinition, id: string) {
  return normalizeWorkflowDepartments({
    ...graph,
    departments: graph.departments?.filter((department) => department.id !== id)
  })
}

export function workflowDepartmentPath(graph: WorkflowDefinition, id: string): string[] {
  const path: string[] = []
  const seen = new Set<string>()
  let current = graph.departments?.find((department) => department.id === id)
  while (current && !seen.has(current.id)) {
    seen.add(current.id)
    path.unshift(current.name)
    current = graph.departments?.find((department) => department.id === current?.parentId)
  }
  return path
}

export function workflowDepartmentLevel(graph: WorkflowDefinition, id: string) {
  return workflowDepartmentPath(graph, id).length
}

/** Move a department and all its descendants together, retaining internal spacing. */
export function moveWorkflowDepartment(
  graph: WorkflowDefinition,
  id: string,
  dx: number,
  dy: number
): WorkflowDefinition {
  const descendants = new Set([id])
  const departments = graph.departments ?? []
  for (let changed = true; changed;) {
    changed = false
    for (const department of departments) {
      if (
        department.parentId &&
        descendants.has(department.parentId) &&
        !descendants.has(department.id)
      ) {
        descendants.add(department.id)
        changed = true
      }
    }
  }
  const next = {
    ...graph,
    departments: departments.map((department) =>
      descendants.has(department.id)
        ? { ...department, x: department.x + dx, y: department.y + dy }
        : department
    ),
    nodes: graph.nodes.map((node) =>
      node.departmentId && descendants.has(node.departmentId)
        ? { ...node, x: node.x + dx, y: node.y + dy }
        : node
    )
  }
  return canPlaceWorkflowDepartments(next.departments) &&
    next.nodes.every(
      (node) =>
        Number.isFinite(node.x) &&
        Math.abs(node.x) <= 100000 &&
        Number.isFinite(node.y) &&
        Math.abs(node.y) <= 100000
    )
    ? normalizeWorkflowDepartments(next)
    : graph
}
