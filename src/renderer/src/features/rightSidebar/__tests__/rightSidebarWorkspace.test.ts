import { describe, expect, it } from 'vitest'
import type { Translate } from '../../../config/translationFormat'
import { RIGHT_SIDEBAR_MODULES } from '../rightSidebarModules'
import {
  INITIAL_RIGHT_SIDEBAR_PLATFORM_STATE,
  reduceRightSidebarPlatform
} from '../rightSidebarPlatformState'
import { createRightSidebarWorkspaceContext } from '../rightSidebarWorkspace'

const translate = ((key: string) => key) as Translate

function createTerminalPage(workspace: ReturnType<typeof createRightSidebarWorkspaceContext>) {
  const terminal = RIGHT_SIDEBAR_MODULES.find((module) => module.id === 'terminal')
  if (!terminal) throw new Error('Missing terminal module')
  const state = reduceRightSidebarPlatform(INITIAL_RIGHT_SIDEBAR_PLATFORM_STATE, {
    module: terminal,
    pageId: 'terminal-page',
    t: translate,
    type: 'open',
    workspace
  })
  const page = state.pages[0]
  if (!page) throw new Error('Terminal page was not created')
  return page
}

describe('right sidebar workspace identity', () => {
  it.each([
    [null, null, undefined, 'home'],
    [null, 'Scratch terminal', undefined, 'Scratch terminal'],
    [null, null, '/tmp/scratch', '/tmp/scratch']
  ] as const)(
    'keeps the %s fallback out of a no-project terminal request',
    (workspaceKey, workspaceName, workspacePath, expectedKey) => {
      const workspace = createRightSidebarWorkspaceContext(
        workspaceKey,
        workspaceName,
        workspacePath
      )
      const page = createTerminalPage(workspace)

      expect(workspace.key).toBe(expectedKey)
      expect(workspace.projectId).toBeNull()
      expect(page).toMatchObject({ projectId: null, workspaceKey: expectedKey, workspacePath })
    }
  )

  it('keeps a selected project id for terminal source validation', () => {
    const workspace = createRightSidebarWorkspaceContext('project-a', 'Project A', '/repo/a')
    const page = createTerminalPage(workspace)

    expect(workspace).toMatchObject({ key: 'project-a', projectId: 'project-a' })
    expect(page).toMatchObject({ projectId: 'project-a', workspaceKey: 'project-a' })
  })
})
