import { createLucideIcon, type IconNode } from 'lucide-react'

// Both directions share the same inbox tray; only the curved delivery arrow changes.
const tray: IconNode = [
  ['path', { d: 'M4 9 2 15v5a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-5l-2-6', key: 'tray-outline' }],
  ['path', { d: 'M2 15h6l2 3h4l2-3h6', key: 'tray-front' }]
]
export const WorkflowInboxIcon = createLucideIcon('WorkflowInbox', [
  ...tray,
  ['path', { d: 'M5 2h3a4 4 0 0 1 4 4v6', key: 'incoming-curve' }],
  ['path', { d: 'm9 9 3 3 3-3', key: 'incoming-tip' }]
])
export const WorkflowOutboxIcon = createLucideIcon('WorkflowOutbox', [
  ...tray,
  ['path', { d: 'M12 12V8a4 4 0 0 1 4-4h3', key: 'outgoing-curve' }],
  ['path', { d: 'm16 1 3 3-3 3', key: 'outgoing-tip' }]
])

export const WorkflowAcceptIcon = createLucideIcon('WorkflowAccept', [
  ...tray,
  ['path', { d: 'M12 12V2m-4 4 4-4 4 4', key: 'take-out' }]
])
export const WorkflowRecallIcon = createLucideIcon('WorkflowRecall', [
  ...tray,
  ['path', { d: 'M8 3h7a4 4 0 0 1 4 4v2M11 0 8 3l3 3', key: 'recall' }]
])
export const WorkflowCompleteIcon = createLucideIcon('WorkflowComplete', [
  ...tray,
  ['path', { d: 'm8 5 3 3 6-6', key: 'complete' }]
])
