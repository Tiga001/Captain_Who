import type { WorkflowDefinition, WorkflowEndpoint } from '@mycopilot/protocol'
import { NODE_HEIGHT, NODE_WIDTH, workflowNodeSize } from './workflowAuthoring'

const INPUT = 'boundary:input',
  OUTPUT = 'boundary:output'
const ROW_GAP = 132,
  COLUMN_GAP = 104,
  GATE_GAP = 32

/** Layer the main flow, keep owned gates beside their agent, and retain cycles as return edges. */
export function arrangeWorkflowNodes(graph: WorkflowDefinition): WorkflowDefinition {
  type Item = {
    id: string
    nodeId?: string
    input?: string
    output?: string
    rank: number
    order: number
  }
  const items: Item[] = [{ id: INPUT, rank: 0, order: 0 }]
  const owner = new Map<string, string>()
  const agents = graph.nodes.filter((node) => node.kind === 'agent' || node.kind === 'user')
  const groups = new Map(
    agents.map((node) => [
      node.id,
      { id: `node:${node.id}`, nodeId: node.id, rank: 1, order: 0 } as Item
    ])
  )
  for (const node of agents) owner.set(node.id, `node:${node.id}`)
  for (const gate of graph.nodes.filter(
    (node) => node.kind === 'inputGate' || node.kind === 'outputGate'
  )) {
    const input = gate.kind === 'inputGate'
    const bindings = graph.flows.filter((flow) => {
      const endpoint = input ? flow.source : flow.target
      return endpoint.kind === 'node' && endpoint.nodeId === gate.id
    })
    const endpoint =
      bindings.length === 1 ? (input ? bindings[0].target : bindings[0].source) : undefined
    const group = endpoint?.kind === 'node' ? groups.get(endpoint.nodeId) : undefined
    const key = input ? 'input' : 'output'
    if (
      group &&
      !group[key] &&
      (input || graph.nodes.find((node) => node.id === group.nodeId)?.kind === 'agent')
    ) {
      group[key] = gate.id
      owner.set(gate.id, group.id)
    }
  }
  for (const node of graph.nodes) {
    if (node.kind === 'agent' || node.kind === 'user') items.push(groups.get(node.id)!)
    else if (!owner.has(node.id)) {
      const id = `node:${node.id}`
      owner.set(node.id, id)
      items.push({ id, nodeId: node.id, rank: 1, order: 0 })
    }
  }
  items.push({ id: OUTPUT, rank: 1, order: 0 })
  items.forEach((item, i) => {
    item.order = i
  })
  const byId = new Map(items.map((item) => [item.id, item]))
  const key = (ep: WorkflowEndpoint, input: boolean) =>
    ep.kind === 'boundary' ? (input ? INPUT : OUTPUT) : owner.get(ep.nodeId)
  const edges = new Map<string, { from: string; to: string; back: boolean }>()
  for (const flow of graph.flows) {
    const from = key(flow.source, true),
      to = key(flow.target, false)
    if (from && to && from !== to) edges.set(`${from}\0${to}`, { from, to, back: false })
  }
  const connections = [...edges.values()].sort(
    (a, b) => byId.get(a.to)!.order - byId.get(b.to)!.order
  )
  // Distance from the entry identifies return edges before DFS. Otherwise a review -> branch
  // return can be visited first and incorrectly push that parallel branch past the review.
  const distance = new Map<string, number>()
  const breadthFirst = (start: string) => {
    const queue = [start]
    distance.set(start, 0)
    for (let i = 0; i < queue.length; i++) {
      for (const edge of connections.filter((edge) => edge.from === queue[i])) {
        if (!distance.has(edge.to)) {
          distance.set(edge.to, distance.get(queue[i])! + 1)
          queue.push(edge.to)
        }
      }
    }
  }
  breadthFirst(INPUT)
  for (const item of items)
    if (!distance.has(item.id) && !connections.some((edge) => edge.to === item.id))
      breadthFirst(item.id)
  for (const item of items) if (!distance.has(item.id)) breadthFirst(item.id)
  const reaches = (from: string, to: string) => {
    const queue = [from],
      seen = new Set(queue)
    for (let i = 0; i < queue.length; i++) {
      if (queue[i] === to) return true
      for (const edge of connections.filter((edge) => edge.from === queue[i])) {
        if (!seen.has(edge.to)) {
          seen.add(edge.to)
          queue.push(edge.to)
        }
      }
    }
    return false
  }
  for (const edge of connections)
    if (distance.get(edge.to)! < distance.get(edge.from)! && reaches(edge.to, edge.from))
      edge.back = true
  const visited = new Set<string>(),
    active = new Set<string>()
  const visit = (id: string) => {
    visited.add(id)
    active.add(id)
    for (const edge of connections.filter((edge) => edge.from === id && !edge.back)) {
      if (active.has(edge.to)) edge.back = true
      else if (!visited.has(edge.to)) visit(edge.to)
    }
    active.delete(id)
  }
  visit(INPUT)
  // Start disconnected draft components at their own entries before visiting remaining cycles.
  for (const item of items)
    if (!visited.has(item.id) && !connections.some((e) => e.to === item.id)) visit(item.id)
  for (const item of items) if (!visited.has(item.id)) visit(item.id)
  const forward = connections.filter((edge) => !edge.back)
  const indegree = new Map(
    items.map((item) => [item.id, forward.filter((e) => e.to === item.id).length])
  )
  const ready = items.filter((item) => !indegree.get(item.id))
  while (ready.length) {
    const item = ready.shift()!
    for (const edge of forward.filter((edge) => edge.from === item.id)) {
      const next = byId.get(edge.to)!
      next.rank = Math.max(next.rank, item.rank + 1)
      indegree.set(next.id, indegree.get(next.id)! - 1)
      if (!indegree.get(next.id)) ready.push(next)
    }
  }
  byId.get(OUTPUT)!.rank =
    Math.max(0, ...items.filter((item) => item.id !== OUTPUT).map((item) => item.rank)) + 1
  const layers = Array.from({ length: byId.get(OUTPUT)!.rank + 1 }, (_, rank) =>
    items.filter((item) => item.rank === rank)
  )
  const rows = () =>
    new Map(
      layers.flatMap((layer) =>
        layer.map((item, i) => [item.id, i - (layer.length - 1) / 2] as const)
      )
    )
  const crossings = () => {
    const positions = rows()
    let count = 0
    for (let i = 0; i < forward.length; i++)
      for (let j = i + 1; j < forward.length; j++) {
        const a = forward[i],
          b = forward[j]
        if (a.from === b.from || a.to === b.to || a.from === b.to || a.to === b.from) continue
        const left = Math.max(byId.get(a.from)!.rank, byId.get(b.from)!.rank)
        const right = Math.min(byId.get(a.to)!.rank, byId.get(b.to)!.rank)
        if (left >= right) continue
        const at = (edge: typeof a, rank: number) => {
          const start = byId.get(edge.from)!.rank,
            end = byId.get(edge.to)!.rank
          return (
            positions.get(edge.from)! +
            ((positions.get(edge.to)! - positions.get(edge.from)!) * (rank - start)) / (end - start)
          )
        }
        if ((at(a, left) - at(b, left)) * (at(a, right) - at(b, right)) < 0) count++
      }
    return count
  }
  let best = layers.map((layer) => [...layer]),
    bestScore = crossings()
  for (let pass = 0; pass < 6; pass++) {
    const downward = pass % 2 === 0
    const ordered = downward ? layers.slice(1) : layers.slice(0, -1).reverse()
    for (const layer of ordered) {
      const positions = rows()
      const center = (item: Item) => {
        const peers = forward.filter((e) => (downward ? e.to === item.id : e.from === item.id))
        return peers.length
          ? peers.reduce((sum, e) => sum + positions.get(downward ? e.from : e.to)!, 0) /
              peers.length
          : positions.get(item.id)!
      }
      layer.sort((a, b) => center(a) - center(b) || a.order - b.order)
    }
    const score = crossings()
    if (score < bestScore) {
      best = layers.map((layer) => [...layer])
      bestScore = score
    }
  }
  const positions = new Map<string, { x: number; y: number }>()
  const rootPositions = { ...graph.boundaryPositions }
  const baseline = 80 + ((Math.max(...best.map((layer) => layer.length)) - 1) * ROW_GAP) / 2
  let x = 48
  const gateWidth = workflowNodeSize({ kind: 'inputGate' }).width
  for (const layer of best) {
    if (!layer.length) continue
    const left = layer.some((item) => item.input) ? gateWidth + GATE_GAP : 0
    const right = layer.some((item) => item.output) ? gateWidth + GATE_GAP : 0
    layer.forEach((item, row) => {
      const cy = baseline + (row - (layer.length - 1) / 2) * ROW_GAP
      const centerX = x + left
      if (item.id === INPUT || item.id === OUTPUT)
        rootPositions[item.id === INPUT ? 'input' : 'output'] = {
          x: centerX,
          y: cy - NODE_HEIGHT / 2
        }
      else {
        const node = graph.nodes.find((node) => node.id === item.nodeId)!
        const size = workflowNodeSize(node)
        positions.set(node.id, {
          x: centerX + (NODE_WIDTH - size.width) / 2,
          y: cy - size.height / 2
        })
        for (const side of ['input', 'output'] as const) {
          const id = item[side]
          if (id)
            positions.set(id, {
              x:
                side === 'input' ? centerX - gateWidth - GATE_GAP : centerX + NODE_WIDTH + GATE_GAP,
              y: cy - workflowNodeSize({ kind: 'inputGate' }).height / 2
            })
        }
      }
    })
    x += left + NODE_WIDTH + right + COLUMN_GAP
  }
  return {
    ...graph,
    nodes: graph.nodes.map((node) => ({ ...node, ...positions.get(node.id) })),
    boundaryPositions: rootPositions
  }
}
