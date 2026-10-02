import type { WorkflowDefinition } from '@mycopilot/protocol'
import { workflowNodeSize } from './workflowAuthoring'

const FRAME_PADDING = 24
const FRAME_HEADER = 40
const MEMBER_GAP = 24
const DEPARTMENT_GAP = 40
const TARGET_ASPECT = 2

interface LayoutBlock {
  id: string
  kind: 'node' | 'department'
  width: number
  height: number
  children?: PositionedBlock[]
}
interface PositionedBlock {
  block: LayoutBlock
  x: number
  y: number
}

/** Try stable shelf arrangements and choose the one that fits a compact landscape view. */
function pack(blocks: LayoutBlock[], framed: boolean) {
  const left = framed ? FRAME_PADDING : 0
  const top = framed ? FRAME_HEADER : 0
  const gap = blocks.some((block) => block.kind === 'department') ? DEPARTMENT_GAP : MEMBER_GAP
  const minimum = workflowNodeSize()
  let best: { children: PositionedBlock[]; width: number; height: number; score: number } | null =
    null

  for (let columns = 1; columns <= Math.max(1, blocks.length); columns++) {
    const children: PositionedBlock[] = []
    let width = 0
    let y = top
    for (let row = 0; row < blocks.length; row += columns) {
      const entries = blocks.slice(row, row + columns)
      let x = left
      const height = Math.max(...entries.map((block) => block.height))
      for (const block of entries) {
        children.push({ block, x, y })
        x += block.width + gap
      }
      width = Math.max(width, x - gap + left)
      y += height + gap
    }
    width = Math.max(width, framed ? minimum.width + FRAME_PADDING * 2 : 0)
    const height = Math.max(
      blocks.length ? y - gap + left : top + left,
      framed ? minimum.height + FRAME_HEADER + FRAME_PADDING : 0
    )
    // The first term minimizes the space needed in a 2:1 viewport. The smaller
    // area breaks ties without producing oversized sparse rows around deep trees.
    const score = Math.max(width / TARGET_ASPECT, height) + Math.sqrt(width * height) * 0.1
    if (!best || score < best.score) best = { children, width, height, score }
  }
  return best!
}

/**
 * Arrange people within their departments, then arrange the resulting department
 * blocks. Only geometry changes: containment follows the existing explicit tree,
 * never the old (possibly overlapping) rectangles. Array order keeps this stable
 * across repeated clicks, including when users have manually overlapped members.
 */
export function optimizeWorkflowLayout(graph: WorkflowDefinition): WorkflowDefinition {
  const departments = graph.departments ?? []
  const byId = new Map(departments.map((department) => [department.id, department]))
  const ids = new Set(graph.nodes.map((node) => node.id))
  if (
    ids.size !== graph.nodes.length ||
    byId.size !== departments.length ||
    departments.some((department) => ids.has(department.id)) ||
    graph.nodes.some((node) => node.departmentId != null && !byId.has(node.departmentId))
  )
    return graph

  // Do not silently repair or discard an invalid hierarchy during a layout action.
  for (const department of departments) {
    const visited = new Set([department.id])
    let parentId = department.parentId
    while (parentId !== null) {
      const parent = byId.get(parentId)
      if (!parent || visited.has(parentId)) return graph
      visited.add(parentId)
      parentId = parent.parentId
    }
  }
  if (!graph.nodes.length && !departments.length) return graph

  function childrenOf(parentId: string | null): LayoutBlock[] {
    return [
      ...graph.nodes
        .filter((node) => (node.departmentId ?? null) === parentId)
        .map((node) => ({ id: node.id, kind: 'node' as const, ...workflowNodeSize(node) })),
      ...departments
        .filter((department) => department.parentId === parentId)
        .map((department) => ({
          id: department.id,
          kind: 'department' as const,
          ...pack(childrenOf(department.id), true)
        }))
    ]
  }

  const positions = new Map<string, { x: number; y: number }>()
  const frames = new Map<string, { x: number; y: number; width: number; height: number }>()
  function place(children: PositionedBlock[], originX: number, originY: number) {
    for (const { block, x, y } of children) {
      const position = { x: originX + x, y: originY + y }
      if (block.kind === 'node') positions.set(block.id, position)
      else {
        frames.set(block.id, { ...position, width: block.width, height: block.height })
        place(block.children ?? [], position.x, position.y)
      }
    }
  }
  place(pack(childrenOf(null), false).children, 0, 0)

  return {
    ...graph,
    nodes: graph.nodes.map((node) => ({ ...node, ...positions.get(node.id) })),
    ...(graph.departments !== undefined
      ? {
          departments: departments.map((department) => ({
            ...department,
            ...frames.get(department.id)
          }))
        }
      : {})
  }
}
