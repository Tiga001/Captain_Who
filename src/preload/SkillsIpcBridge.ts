import type { IpcRenderer } from 'electron'
import type { SkillsHostApi } from '@mycopilot/host-api'

const SKILLS_LIST_CHANNEL = 'host:skills.list'

/** Keep the preload boundary transport-only; schema capability checks belong to the renderer. */
export function createSkillsIpcBridge(ipcRenderer: Pick<IpcRenderer, 'invoke'>): SkillsHostApi {
  return {
    list: (input) => ipcRenderer.invoke(SKILLS_LIST_CHANNEL, input)
  }
}
