import { describe, expect, it } from 'vitest'
import type { WorkflowDepartment } from '@mycopilot/protocol'
import { createWorkflow, createWorkflowNode } from '../../features/workflows/workflowAuthoring'
import { mergeWorkflowDefinition } from '../../features/workflows/workflowDefinitionMerge'

describe('organization edit refresh', () => {
  it('retains local field edits and merges independent remote member changes', () => {
    const base = {
      ...createWorkflow(),
      nodes: [createWorkflowNode('Planner', 0, 0), createWorkflowNode('Reviewer', 300, 0)]
    }
    const local = structuredClone(base)
    local.description = 'Local purpose'
    local.nodes[0].task = 'Local assignment'
    const remote = structuredClone(base)
    remote.nodes[0].rank = 5
    remote.nodes[1].name = 'Senior reviewer'
    remote.nodes.push(createWorkflowNode('Researcher', 600, 0))
    const { definition: merged, removedEntities } = mergeWorkflowDefinition(base, local, remote)
    expect(merged.description).toBe('Local purpose')
    expect(merged.nodes).toHaveLength(3)
    expect(merged.nodes[0]).toMatchObject({ task: 'Local assignment', rank: 5 })
    expect(merged.nodes[1].name).toBe('Senior reviewer')
    expect(removedEntities).toEqual([])
  })
  it('keeps local additions and deletions while applying remote deletions of untouched members', () => {
    const base = {
      ...createWorkflow(),
      nodes: [createWorkflowNode('A', 0, 0), createWorkflowNode('B', 300, 0)]
    }
    const added = createWorkflowNode('C', 600, 0)
    const local = { ...base, nodes: [base.nodes[0], added] }
    const remote = { ...base, nodes: [base.nodes[1]] }
    expect(mergeWorkflowDefinition(base, local, remote)).toMatchObject({
      definition: { nodes: [added] },
      removedEntities: []
    })
  })
  it('reports edited members deleted remotely without resurrecting them or discarding unrelated edits', () => {
    const base = {
      ...createWorkflow(),
      nodes: [createWorkflowNode('Planner', 0, 0), createWorkflowNode('Reviewer', 300, 0)]
    }
    const local = structuredClone(base)
    local.nodes[0].task = 'Unsaved assignment'
    local.nodes[1].task = 'Retained assignment'
    local.background = 'Retained context'
    const remote = { ...base, nodes: [base.nodes[1]] }
    const merged = mergeWorkflowDefinition(base, local, remote)
    expect(merged.definition.nodes).toEqual([local.nodes[1]])
    expect(merged.definition.background).toBe('Retained context')
    expect(merged.removedEntities).toEqual([
      { kind: 'member', id: base.nodes[0].id, name: 'Planner' }
    ])
  })
  it('keeps removed departments deleted and promotes retained additions to the surviving ancestor', () => {
    const parent: WorkflowDepartment = {
      id: 'parent',
      name: 'Engineering',
      parentId: null,
      x: 0,
      y: 0,
      width: 1000,
      height: 700
    }
    const removed: WorkflowDepartment = {
      ...parent,
      id: 'removed',
      name: 'Research',
      parentId: parent.id,
      x: 20,
      y: 40
    }
    const base = { ...createWorkflow(), departments: [parent, removed], nodes: [] }
    const local = structuredClone(base)
    local.departments[1].name = 'Local research name'
    const added = { ...createWorkflowNode('New researcher', 40, 80), departmentId: removed.id }
    const newDepartment = { ...removed, id: 'local', name: 'Local team', parentId: removed.id }
    const merged = mergeWorkflowDefinition(
      base,
      { ...local, nodes: [added], departments: [...local.departments, newDepartment] },
      { ...base, departments: [parent] }
    )
    expect(merged.definition.departments).toEqual([
      parent,
      { ...newDepartment, parentId: parent.id }
    ])
    expect(merged.definition.nodes).toEqual([{ ...added, departmentId: parent.id }])
    expect(merged.removedEntities).toEqual([
      { kind: 'department', id: removed.id, name: 'Local research name' }
    ])
  })
  it('retains local members at root without an invalid department administrator role when their scope was removed', () => {
    const removed: WorkflowDepartment = {
      id: 'removed',
      name: 'Research',
      parentId: null,
      x: 0,
      y: 0,
      width: 1000,
      height: 700
    }
    const base = { ...createWorkflow(), departments: [removed], nodes: [] }
    const added = {
      ...createWorkflowNode('New researcher', 40, 80),
      departmentId: removed.id,
      managementRole: 'department_admin' as const
    }
    const merged = mergeWorkflowDefinition(
      base,
      { ...base, nodes: [added] },
      { ...base, departments: [] }
    )
    expect(merged.definition.nodes).toEqual([
      { ...added, departmentId: null, managementRole: 'member' }
    ])
    expect(merged.definition.departments).toEqual([])
    expect(merged.removedEntities).toEqual([
      { kind: 'department', id: removed.id, name: 'Research' }
    ])
  })
  it('resolves a local administrator promotion when a remote deletion already moved its member to root', () => {
    const department: WorkflowDepartment = {
      id: 'removed',
      name: 'Research',
      parentId: null,
      x: 0,
      y: 0,
      width: 1000,
      height: 700
    }
    const member = { ...createWorkflowNode('Researcher', 40, 80), departmentId: department.id }
    const base = { ...createWorkflow(), departments: [department], nodes: [member] }
    const local = {
      ...base,
      nodes: [
        { ...member, task: 'Retained assignment', managementRole: 'department_admin' as const }
      ]
    }
    const remote = { ...base, departments: [], nodes: [{ ...member, departmentId: null }] }
    const merged = mergeWorkflowDefinition(base, local, remote)
    expect(merged.definition.nodes).toEqual([
      { ...member, departmentId: null, managementRole: 'member', task: 'Retained assignment' }
    ])
    expect(merged.removedEntities).toEqual([
      { kind: 'department', id: department.id, name: 'Research' }
    ])
  })
})
