import { createLucideIcon } from 'lucide-react'

/** Two overlapping agents, using the same outline style as the surrounding Lucide icons. */
export const BotGroupIcon = createLucideIcon('BotGroup', [
  ['path', { d: 'M10 7V5a2 2 0 0 1 2-2h7a2 2 0 0 1 2 2v6a2 2 0 0 1-2 2h-2', key: 'rear' }],
  ['path', { d: 'M15 3V1m8 5v4M14 6v1m4-1v1', key: 'rear-features' }],
  ['rect', { x: '3', y: '10', width: '12', height: '10', rx: '2', key: 'front' }],
  ['path', { d: 'M9 10V7H7M1 13v4m16-4v4', key: 'front-antenna-ears' }],
  ['path', { d: 'M7 14v1m4-1v1m-4 2h4', key: 'front-face' }]
])
