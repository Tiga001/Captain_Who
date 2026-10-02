import { describe, expect, it } from 'vitest'
import { createWorkflow, createWorkflowNode } from '../../features/workflows/workflowAuthoring'
import {
  addWorkflowDepartment,
  canPlaceWorkflowDepartments,
  moveWorkflowDepartment,
  normalizeWorkflowDepartments,
  removeWorkflowDepartment,
  workflowDepartmentLevel,
  workflowDepartmentPath,
  type WorkflowDepartment
} from '../../features/workflows/workflowDepartments'
import { graphBounds } from '../../features/workflows/workflowCanvasGeometry'

const department = (
  id: string,
  x: number,
  y: number,
  width: number,
  height: number
): WorkflowDepartment => ({
  id,
  name: id,
  x,
  y,
  width,
  height,
  parentId: null
})

function nestedOrganization() {
  const graph = createWorkflow()
  graph.departments = [
    department('Research', 100, 100, 800, 600),
    department('Analysis', 150, 200, 500, 300)
  ]
  graph.nodes = [
    { ...createWorkflowNode('Analyst', 200, 250), managementRole: 'department_admin' },
    createWorkflowNode('Director', 400, 550),
    createWorkflowNode('Independent', 1000, 250)
  ]
  return normalizeWorkflowDepartments(graph)
}

describe('organization template departments', () => {
  it('assigns available default names after inferring the new department parent', () => {
    let graph = createWorkflow()
    graph = addWorkflowDepartment(graph, department('New department', 0, 0, 800, 800))
    const sibling = { ...department('sibling', 900, 0, 800, 800), name: 'NEW DEPARTMENT' }
    graph = addWorkflowDepartment(graph, sibling)
    expect(graph.departments?.[1].name).toBe('NEW DEPARTMENT (2)')
    const nested = { ...department('nested', 100, 100, 500, 500), name: 'New department' }
    graph = addWorkflowDepartment(graph, nested)
    expect(graph.departments?.[2]).toMatchObject({
      name: 'New department',
      parentId: 'New department'
    })
  })

  it('uses the innermost complete containment, with explicit ancestors and levels', () => {
    const graph = nestedOrganization()
    expect(graph.departments?.map((item) => [item.id, item.parentId])).toEqual([
      ['Research', null],
      ['Analysis', 'Research']
    ])
    expect(graph.nodes.map((node) => node.departmentId)).toEqual(['Analysis', 'Research', null])
    expect(workflowDepartmentPath(graph, 'Analysis')).toEqual(['Research', 'Analysis'])
    expect(workflowDepartmentLevel(graph, 'Analysis')).toBe(2)
    expect(graph.nodes[0].managementRole).toBe('department_admin')
    graph.nodes[0].x = 500 // center belongs to Analysis but its full card extends beyond the frame
    expect(normalizeWorkflowDepartments(graph).nodes[0].departmentId).toBe('Research')
  })

  it('rejects partial overlap and duplicate frames while allowing nested and adjacent frames', () => {
    const first = department('A', 0, 0, 500, 500)
    expect(canPlaceWorkflowDepartments([first, department('B', 50, 50, 100, 100)])).toBe(true)
    expect(canPlaceWorkflowDepartments([first, department('B', 500, 0, 100, 100)])).toBe(true)
    expect(canPlaceWorkflowDepartments([first, department('B', 400, 400, 200, 200)])).toBe(false)
    expect(canPlaceWorkflowDepartments([first, department('B', 0, 0, 500, 500)])).toBe(false)
  })

  it('moves a department subtree and its members without moving unrelated members', () => {
    const graph = nestedOrganization()
    const next = moveWorkflowDepartment(graph, 'Research', 300, 400)
    expect(next.departments?.map((item) => [item.x, item.y])).toEqual([
      [400, 500],
      [450, 600]
    ])
    expect(next.nodes.map((node) => [node.x, node.y])).toEqual([
      [500, 650],
      [700, 950],
      [1000, 250]
    ])
    expect(next.nodes[0].managementRole).toBe('department_admin')
    expect(next.nodes[0].departmentId).toBe('Analysis')
  })

  it('rejects an invalid department move without altering members or the existing hierarchy', () => {
    const graph = nestedOrganization()
    expect(moveWorkflowDepartment(graph, 'Analysis', 500, 0)).toBe(graph)
  })

  it('keeps unassigned defaults absent and rejects movement beyond a member coordinate limit', () => {
    const graph = createWorkflow()
    graph.nodes = [createWorkflowNode('Independent', 0, 0)]
    delete graph.nodes[0].departmentId
    expect(normalizeWorkflowDepartments(graph).nodes[0]).not.toHaveProperty('departmentId')
    graph.departments = [department('Wide', 99900, 0, 1000, 100)]
    graph.nodes[0].x = 99950
    const assigned = normalizeWorkflowDepartments(graph)
    expect(moveWorkflowDepartment(assigned, 'Wide', 100, 0)).toBe(assigned)
    const negative = moveWorkflowDepartment(assigned, 'Wide', -100000, 0)
    expect(negative.nodes[0]).toMatchObject({ x: -50, departmentId: 'Wide' })
  })

  it('promotes nodes and child departments when a frame is removed, retaining conversations', () => {
    const graph = nestedOrganization()
    const innerRemoved = removeWorkflowDepartment(graph, 'Analysis')
    expect(innerRemoved.nodes).toHaveLength(3)
    expect(innerRemoved.nodes[0]).toMatchObject({
      departmentId: 'Research',
      managementRole: 'department_admin'
    })
    const outerRemoved = removeWorkflowDepartment(graph, 'Research')
    expect(outerRemoved.departments?.[0]).toMatchObject({ id: 'Analysis', parentId: null })
    expect(outerRemoved.nodes[0].departmentId).toBe('Analysis')
    expect(outerRemoved.nodes[1].departmentId).toBeNull()
    const allRemoved = removeWorkflowDepartment(innerRemoved, 'Research')
    expect(allRemoved.nodes[0]).toMatchObject({ departmentId: null, managementRole: 'member' })
  })

  it('downgrades department administrators moved outside, while keeping organization administrators', () => {
    const graph = nestedOrganization()
    graph.nodes[0].x = 2000
    graph.nodes[1].managementRole = 'organization_admin'
    graph.nodes[1].x = 3000
    const next = normalizeWorkflowDepartments(graph)
    expect(next.nodes[0]).toMatchObject({ departmentId: null, managementRole: 'member' })
    expect(next.nodes[1]).toMatchObject({
      departmentId: null,
      managementRole: 'organization_admin'
    })
  })

  it('includes empty departments in the canvas bounds and fit view', () => {
    const graph = createWorkflow()
    graph.departments = [department('Empty', -100, -200, 1200, 800)]
    expect(graphBounds(graph)).toEqual({ left: -104, top: -204, right: 1104, bottom: 604 })
  })
})
