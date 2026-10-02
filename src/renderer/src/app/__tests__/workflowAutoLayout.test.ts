import { describe, expect, it } from 'vitest'
import { parseWorkflowDefinition, type WorkflowDefinition } from '@mycopilot/protocol'
import {
  createWorkflow,
  createWorkflowNode,
  workflowNodeSize
} from '../../features/workflows/workflowAuthoring'
import { optimizeWorkflowLayout } from '../../features/workflows/workflowAutoLayout'
import { normalizeWorkflowDepartments } from '../../features/workflows/workflowDepartments'

const department = (id: string, parentId: string | null = null) => ({
  id,
  parentId,
  name: `Department ${id}`,
  x: -400,
  y: -400,
  width: 1000,
  height: 1000
})

const configuration = (value: object) =>
  Object.fromEntries(
    Object.entries(value).filter(([key]) => !['x', 'y', 'width', 'height'].includes(key))
  )

function expectValidLayout(graph: WorkflowDefinition) {
  const departments = graph.departments ?? []
  const rectangles = [
    ...departments,
    ...graph.nodes.map((node) => ({ ...node, ...workflowNodeSize(node) }))
  ]
  const parents = new Map([
    ...departments.map((item) => [item.id, item.parentId] as const),
    ...graph.nodes.map((node) => [node.id, node.departmentId ?? null] as const)
  ])
  const ancestorOf = (ancestor: string, id: string) => {
    let parent = parents.get(id)
    while (parent) {
      if (parent === ancestor) return true
      parent = parents.get(parent)
    }
    return false
  }
  rectangles.forEach((first, index) => {
    expect([first.x, first.y, first.width, first.height].every(Number.isFinite)).toBe(true)
    expect(Math.max(first.x + first.width, first.y + first.height)).toBeLessThan(100_000)
    for (const second of rectangles.slice(index + 1)) {
      if (ancestorOf(first.id, second.id) || ancestorOf(second.id, first.id)) continue
      const overlaps =
        first.x < second.x + second.width &&
        first.x + first.width > second.x &&
        first.y < second.y + second.height &&
        first.y + first.height > second.y
      expect(overlaps, `${first.id} overlaps ${second.id}`).toBe(false)
    }
    const parentId = parents.get(first.id)
    if (!parentId) return
    const parent = departments.find((item) => item.id === parentId)!
    expect(first.x).toBeGreaterThan(parent.x)
    expect(first.y).toBeGreaterThan(parent.y + 20)
    expect(first.x + first.width).toBeLessThan(parent.x + parent.width)
    expect(first.y + first.height).toBeLessThan(parent.y + parent.height)
  })
  // Later drag/edit normalization must see exactly the same memberships.
  const normalized = normalizeWorkflowDepartments(graph)
  expect(normalized.nodes.map((node) => node.departmentId)).toEqual(
    graph.nodes.map((node) => node.departmentId)
  )
  expect(normalized.departments?.map((item) => item.parentId) ?? []).toEqual(
    departments.map((item) => item.parentId)
  )
}

describe('organization automatic layout', () => {
  it('separates overlapping people inside their department and shrinks the frame', () => {
    const graph = createWorkflow()
    graph.departments = [department('hr')]
    graph.nodes = [
      { ...createWorkflowNode('One', 0, 0), departmentId: 'hr' },
      { ...createWorkflowNode('Two', 10, 10), departmentId: 'hr' }
    ]
    const result = optimizeWorkflowLayout(graph)
    expectValidLayout(result)
    expect(result.departments![0].width * result.departments![0].height).toBeLessThan(200_000)
  })

  it('preserves the explicit nested hierarchy, rank, permissions, roles and all content', () => {
    const graph = createWorkflow()
    graph.viewport = { x: 734, y: 121, zoom: 0.75 }
    graph.departments = [
      department('research'),
      department('hr'),
      department('analysis', 'research'),
      department('testing', 'analysis'),
      department('empty', 'research')
    ]
    graph.nodes = [
      { ...createWorkflowNode('Owner', 999, 999), rank: 99, managementRole: 'organization_admin' },
      {
        ...createWorkflowNode('Researcher', 999, 999, 'specific-model'),
        departmentId: 'research',
        managementRole: 'department_admin',
        rank: 9,
        receives: 'Research requests',
        task: 'Analyze the material',
        delivers: 'A report',
        permissionMode: 'custom'
      },
      { ...createWorkflowNode('Analyst', 999, 999), departmentId: 'analysis' },
      { ...createWorkflowNode('Tester', 999, 999), departmentId: 'testing' },
      { ...createWorkflowNode('HR', 999, 999), departmentId: 'hr' }
    ]
    const original = structuredClone(graph)
    const result = optimizeWorkflowLayout(graph)
    expect(graph).toEqual(original)
    expectValidLayout(result)
    expect(result.viewport).toBe(graph.viewport)
    expect(result.nodes.map(configuration)).toEqual(graph.nodes.map(configuration))
    expect(result.departments!.map(configuration)).toEqual(graph.departments.map(configuration))
    expect(optimizeWorkflowLayout(result)).toEqual(result)
    expect(parseWorkflowDefinition(result)).toEqual(result)
  })

  it('is deterministic and compact for a flat collection, independent of old positions', () => {
    const graph = createWorkflow()
    delete graph.departments
    graph.nodes = Array.from({ length: 12 }, (_, index) => ({
      ...createWorkflowNode(`Member ${index}`, index * 700, -index * 800),
      id: `member-${index}`
    }))
    const result = optimizeWorkflowLayout(graph)
    expectValidLayout(result)
    expect(result).not.toHaveProperty('departments')
    expect(Math.max(...result.nodes.map((node) => node.x))).toBeLessThan(1000)
    expect(Math.max(...result.nodes.map((node) => node.y))).toBeLessThan(1000)
    expect(optimizeWorkflowLayout(result)).toEqual(result)
    expect(
      optimizeWorkflowLayout({
        ...graph,
        nodes: graph.nodes.map((node) => ({ ...node, x: 100, y: 100 }))
      })
    ).toEqual(result)
  })

  it('arranges empty departments as useful visible frames, even without members', () => {
    const graph = createWorkflow()
    graph.departments = [department('a'), department('b'), department('c', 'b')]
    const result = optimizeWorkflowLayout(graph)
    expectValidLayout(result)
    expect(result.departments).toHaveLength(3)
    expect(result.departments!.every((item) => item.width >= 212 && item.height >= 100)).toBe(true)
    expect(optimizeWorkflowLayout(result)).toEqual(result)
  })

  it.each(['nested', 'siblings'] as const)(
    'keeps maximum-sized %s organizations finite, non-overlapping and repeatable',
    (shape) => {
      const graph = createWorkflow()
      graph.departments = Array.from({ length: 64 }, (_, index) =>
        department(`dept-${index}`, shape === 'nested' && index > 0 ? `dept-${index - 1}` : null)
      )
      graph.nodes = Array.from({ length: 128 }, (_, index) => ({
        ...createWorkflowNode(`Member ${index}`, 0, 0),
        departmentId: `dept-${index % 64}`
      }))
      const result = optimizeWorkflowLayout(graph)
      expectValidLayout(result)
      expect(optimizeWorkflowLayout(result)).toEqual(result)
      expect(parseWorkflowDefinition(result)).toEqual(result)
    }
  )

  it('leaves malformed trees intact instead of reassigning or dropping members', () => {
    const graph = createWorkflow()
    graph.departments = [department('a', 'b'), department('b', 'a')]
    graph.nodes = [{ ...createWorkflowNode('One', 20, 20), departmentId: 'a' }]
    expect(optimizeWorkflowLayout(graph)).toBe(graph)
    graph.departments = [department('a', 'missing')]
    expect(optimizeWorkflowLayout(graph)).toBe(graph)
    graph.departments = []
    expect(optimizeWorkflowLayout(graph)).toBe(graph)
    graph.departments = [department('a'), department('a')]
    expect(optimizeWorkflowLayout(graph)).toBe(graph)
  })

  it('preserves an entirely empty definition', () => {
    const graph = createWorkflow()
    expect(optimizeWorkflowLayout(graph)).toBe(graph)
  })
})
