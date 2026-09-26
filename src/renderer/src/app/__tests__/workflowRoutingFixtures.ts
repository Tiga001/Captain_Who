import {
  createWorkflow,
  createWorkflowGate,
  createWorkflowNode
} from '../../features/workflows/workflowAuthoring'
import type { WorkflowDefinition, WorkflowFlow, WorkflowAnchor } from '@mycopilot/protocol'

/** Parallel implementation, review, and two return loops with user-placed anchors. */
export function reviewLoopGraph(): WorkflowDefinition {
  const agent = (id: string, name: string, x: number, y: number) => ({
    ...createWorkflowNode(name, x, y, 'model'),
    id
  })
  const gate = (id: string, kind: 'inputGate' | 'outputGate', x: number, y: number) => ({
    ...createWorkflowGate(kind, x, y),
    id
  })
  const anchor = (offset: number): WorkflowAnchor => ({ side: 'left', offset })
  const edge = (
    id: string,
    source: string | null,
    target: string | null,
    sourceAnchor?: WorkflowAnchor,
    targetAnchor?: WorkflowAnchor
  ): WorkflowFlow => ({
    id,
    name: '',
    source: source ? { kind: 'node', nodeId: source } : { kind: 'boundary' },
    target: target ? { kind: 'node', nodeId: target } : { kind: 'boundary' },
    ...(sourceAnchor ? { sourceAnchor } : {}),
    ...(targetAnchor ? { targetAnchor } : {})
  })
  return {
    ...createWorkflow(),
    id: 'routing-review',
    name: '分支协作与返工',
    nodes: [
      agent('plan', '任务拆解', 0, 220),
      gate('split', 'outputGate', 220, 220),
      gate('input-a', 'inputGate', 250, 10),
      agent('a', '开发工程师 A', 350, 120),
      agent('b', '开发工程师 B', 350, 220),
      agent('c', '开发工程师 C', 350, 320),
      gate('input-c', 'inputGate', 420, 515),
      gate('merge', 'inputGate', 620, 220),
      agent('review', 'Review 专家', 735, 250),
      gate('output', 'outputGate', 1020, 250)
    ],
    boundaryPositions: { input: { x: -280, y: 220 }, output: { x: 1190, y: 250 } },
    flows: [
      edge('entry', null, 'plan'),
      edge('plan-split', 'plan', 'split'),
      edge('split-a', 'split', 'input-a', { side: 'right', offset: 0.2 }, anchor(0.75)),
      edge('split-b', 'split', 'b', { side: 'right', offset: 0.5 }),
      edge('split-c', 'split', 'input-c', { side: 'right', offset: 0.8 }, anchor(0.8)),
      edge('a-binding', 'input-a', 'a'),
      edge('c-binding', 'input-c', 'c', undefined, { side: 'bottom', offset: 0.55 }),
      edge('a-merge', 'a', 'merge', undefined, anchor(0.2)),
      edge('b-merge', 'b', 'merge', undefined, anchor(0.5)),
      edge('c-merge', 'c', 'merge', undefined, anchor(0.8)),
      edge('review-binding', 'merge', 'review'),
      edge('review-output', 'review', 'output'),
      edge('return-a', 'output', 'input-a', { side: 'right', offset: 0.2 }, anchor(0.25)),
      edge('return-c', 'output', 'input-c', { side: 'right', offset: 0.8 }, anchor(0.2)),
      edge('exit', 'output', null, { side: 'right', offset: 0.5 })
    ]
  }
}
