import type { MyCopilotGlobal } from '@mycopilot/host-api'

declare global {
  interface Window {
    mycopilot: MyCopilotGlobal
  }
}
