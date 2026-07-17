import { describe, expect, it } from 'vitest'
import type { Translate } from '../../../config/translationFormat'
import {
  resolveRightSidebarModuleAvailability,
  resolveRightSidebarModuleAvailabilityMap
} from '../rightSidebarModuleAvailability'
import {
  INITIAL_RIGHT_SIDEBAR_PLATFORM_STATE,
  reduceRightSidebarPlatform,
  type RightSidebarPlatformState
} from '../rightSidebarPlatformState'
import type {
  RightSidebarModuleDefinition,
  RightSidebarModuleId,
  RightSidebarWorkspaceContext
} from '../rightSidebarTypes'
import { createRightSidebarWorkspaceContext } from '../rightSidebarWorkspace'

const translate = ((key: string) => key) as Translate
const TestIcon = (() => null) as unknown as RightSidebarModuleDefinition['icon']

const MODULES: RightSidebarModuleDefinition[] = [
  createModule('terminal', 'pinned-to-creation-workspace', 'multiple', 'retain-page'),
  createModule('browser', 'global', 'multiple', 'retain-page'),
  {
    ...createModule('files', 'pinned-to-creation-workspace', 'multiple', 'close-page'),
    maxRelatedPagesPerWorkspace: 2,
    orphanedWorkspacePolicy: 'close-page',
    requiresWorkspace: true,
    retention: 'unmount-when-inactive'
  },
  {
    ...createModule('git-review', 'follow-workspace', 'single', 'close-page'),
    requiredCapability: 'git-repository'
  }
]

const WORKSPACE_A = createRightSidebarWorkspaceContext('project-a', 'Project A', '/repo/a')
const WORKSPACE_B = createRightSidebarWorkspaceContext('project-b', 'Project B', '/repo/b')
const HOME = createRightSidebarWorkspaceContext(null, null, undefined)

describe('right sidebar platform context lifecycle', () => {
  it('reuses an empty file source page once, then opens independent previews', () => {
    const initial = openPages(WORKSPACE_A, ['files'])
    const firstPreview = reduceRightSidebarPlatform(initial, {
      module: getModule('files'),
      pageId: 'unused-file-page',
      request: {
        disposition: 'reuse-source-if-empty',
        iconUrl: 'file:///markdown.svg',
        moduleState: { kind: 'workspace-file', path: 'README.md' },
        resourceKey: 'workspace-file:README.md',
        title: 'README.md'
      },
      sourcePageId: 'files-page',
      type: 'open-related-page'
    })

    expect(firstPreview.activePageId).toBe('files-page')
    expect(firstPreview.pages).toHaveLength(1)
    expect(firstPreview.pages[0]).toMatchObject({
      iconUrl: 'file:///markdown.svg',
      id: 'files-page',
      moduleState: { kind: 'workspace-file', path: 'README.md' },
      resourceKey: 'workspace-file:README.md',
      title: 'README.md',
      workspaceKey: WORKSPACE_A.key,
      workspaceSessionKey: WORKSPACE_A.sessionKey
    })

    const secondPreview = reduceRightSidebarPlatform(firstPreview, {
      module: getModule('files'),
      pageId: 'second-file-page',
      request: {
        disposition: 'new-page',
        moduleState: { kind: 'workspace-file', path: 'src/index.ts' },
        resourceKey: 'workspace-file:src/index.ts',
        title: 'index.ts'
      },
      sourcePageId: 'files-page',
      type: 'open-related-page'
    })

    expect(secondPreview.activePageId).toBe('second-file-page')
    expect(secondPreview.pages.map((page) => page.id)).toEqual(['files-page', 'second-file-page'])
  })

  it('opens related resources as independent pages and reuses an existing resource page', () => {
    const initial = openPages(WORKSPACE_A, ['files'])
    const opened = reduceRightSidebarPlatform(initial, {
      module: getModule('files'),
      pageId: 'workspace-file-page',
      request: {
        iconUrl: 'file:///python.svg',
        moduleState: { kind: 'workspace-file', path: 'src/index.py' },
        resourceKey: 'workspace-file:src/index.py',
        title: 'index.py'
      },
      sourcePageId: 'files-page',
      type: 'open-related-page'
    })

    expect(opened.activePageId).toBe('workspace-file-page')
    expect(opened.pages).toHaveLength(2)
    expect(opened.pages[1]).toMatchObject({
      iconUrl: 'file:///python.svg',
      moduleId: 'files',
      moduleState: { kind: 'workspace-file', path: 'src/index.py' },
      resourceKey: 'workspace-file:src/index.py',
      title: 'index.py',
      workspaceKey: WORKSPACE_A.key,
      workspaceSessionKey: WORKSPACE_A.sessionKey
    })

    const reopened = reduceRightSidebarPlatform(opened, {
      module: getModule('files'),
      pageId: 'duplicate-file-page',
      request: {
        moduleState: { kind: 'workspace-file', path: 'src/index.py' },
        resourceKey: 'workspace-file:src/index.py',
        title: 'index.py'
      },
      sourcePageId: 'files-page',
      type: 'open-related-page'
    })

    expect(reopened).toBe(opened)
  })

  it('keeps files pinned to their creation workspace while review follows the conversation', () => {
    const initial = openPages(WORKSPACE_A, ['files', 'terminal', 'browser', 'git-review'])
    const [filesBefore, terminalBefore, browserBefore, reviewBefore] = initial.pages

    const next = synchronize(initial, WORKSPACE_B, 'available', [WORKSPACE_A.key, WORKSPACE_B.key])

    expect(next.pages[0]).toBe(filesBefore)
    expect(next.pages[1]).toBe(terminalBefore)
    expect(next.pages[2]).toBe(browserBefore)
    expect(next.pages[3]).not.toBe(reviewBefore)
    expect(next.pages[0]).toMatchObject({
      workspaceKey: WORKSPACE_A.key,
      workspacePath: WORKSPACE_A.path,
      workspaceSessionKey: WORKSPACE_A.sessionKey
    })
    expect(next.pages[3]).toMatchObject({
      workspaceKey: WORKSPACE_B.key,
      workspacePath: WORKSPACE_B.path,
      workspaceSessionKey: WORKSPACE_B.sessionKey
    })
  })

  it('keeps pinned files outside a conversation until their project is deleted', () => {
    const initial = openPages(WORKSPACE_A, ['files', 'terminal', 'browser'])
    const filesBefore = initial.pages[0]

    const withoutConversation = synchronize(initial, HOME, 'unavailable', [WORKSPACE_A.key])
    expect(withoutConversation.pages[0]).toBe(filesBefore)

    const afterProjectDeletion = synchronize(withoutConversation, HOME, 'unavailable', [])
    expect(afterProjectDeletion.pages.map((page) => page.moduleId)).toEqual(['terminal', 'browser'])
    expect(afterProjectDeletion.pages[0]).toBe(initial.pages[1])
    expect(afterProjectDeletion.pages[1]).toBe(initial.pages[2])
  })

  it('evicts only the oldest inactive preview after the explicit workspace limit', () => {
    let state = openPages(WORKSPACE_A, ['files', 'terminal'])
    state = openFilePage(state, 'a.ts')
    state = reduceRightSidebarPlatform(state, { pageId: 'files-page', type: 'activate' })
    state = openFilePage(state, 'b.ts')
    state = reduceRightSidebarPlatform(state, { pageId: 'files-page', type: 'activate' })
    state = openFilePage(state, 'c.ts')

    expect(state.activePageId).toBe('file-c.ts')
    expect(state.pages.map((page) => page.id)).toEqual([
      'files-page',
      'terminal-page',
      'file-b.ts',
      'file-c.ts'
    ])
  })

  it('stores preview state on the lightweight file page descriptor', () => {
    const opened = openFilePage(openPages(WORKSPACE_A, ['files']), 'README.md')
    const moduleState = {
      kind: 'workspace-file' as const,
      path: 'README.md',
      preview: { markdownView: 'preview' as const, wrapLines: true }
    }
    const updated = reduceRightSidebarPlatform(opened, {
      pageId: 'file-README.md',
      type: 'update',
      update: { moduleState }
    })

    expect(updated.pages.at(-1)?.moduleState).toBe(moduleState)
  })

  it('rebinds only follow-workspace pages while preserving tab identity and pinned surfaces', () => {
    const initial = openPages(WORKSPACE_A, ['terminal', 'git-review', 'browser'])
    const active = reduceRightSidebarPlatform(initial, {
      pageId: 'git-review-page',
      type: 'activate'
    })
    const [terminalBefore, reviewBefore, browserBefore] = active.pages

    const next = synchronize(active, WORKSPACE_B, 'checking')

    expect(next.pages.map((page) => page.id)).toEqual([
      'terminal-page',
      'git-review-page',
      'browser-page'
    ])
    expect(next.activePageId).toBe('git-review-page')
    expect(next.pages[0]).toBe(terminalBefore)
    expect(next.pages[2]).toBe(browserBefore)
    expect(next.pages[1]).not.toBe(reviewBefore)
    expect(next.pages[1]).toMatchObject({
      workspaceKey: 'project-b',
      workspacePath: '/repo/b',
      workspaceSessionKey: WORKSPACE_B.sessionKey
    })
  })

  it('does not remount pages when the workspace identity is unchanged', () => {
    const initial = openPages(WORKSPACE_A, ['terminal', 'git-review', 'browser'])

    expect(synchronize(initial, WORKSPACE_A, 'available')).toBe(initial)
  })

  it('closes only unavailable follow pages and activates the neighboring tab', () => {
    const initial = openPages(WORKSPACE_A, ['terminal', 'git-review', 'browser'])
    const active = reduceRightSidebarPlatform(initial, {
      pageId: 'git-review-page',
      type: 'activate'
    })
    const terminalBefore = active.pages[0]
    const browserBefore = active.pages[2]

    const next = synchronize(active, WORKSPACE_B, 'unavailable')

    expect(next.pages).toEqual([terminalBefore, browserBefore])
    expect(next.activePageId).toBe('browser-page')
  })

  it('closes review without touching other pages when no workspace is active', () => {
    const initial = openPages(WORKSPACE_A, ['git-review', 'terminal', 'browser'])
    const terminalBefore = initial.pages[1]
    const browserBefore = initial.pages[2]

    const next = synchronize(initial, HOME, 'unavailable')

    expect(next.pages).toEqual([terminalBefore, browserBefore])
  })

  it('keeps review globally single after it follows the new workspace', () => {
    const rebound = synchronize(openPages(WORKSPACE_A, ['git-review']), WORKSPACE_B, 'available')
    const reviewModule = getModule('git-review')

    const reopened = reduceRightSidebarPlatform(rebound, {
      module: reviewModule,
      pageId: 'another-review-page',
      t: translate,
      type: 'open',
      workspace: WORKSPACE_B
    })

    expect(reopened.pages).toHaveLength(1)
    expect(reopened.pages[0]?.id).toBe('git-review-page')
  })

  it('pins existing terminals to their creation workspace while new terminals use the new one', () => {
    const initial = openPages(WORKSPACE_A, ['terminal'])

    const next = reduceRightSidebarPlatform(initial, {
      module: getModule('terminal'),
      pageId: 'terminal-b-page',
      t: translate,
      type: 'open',
      workspace: WORKSPACE_B
    })

    expect(next.pages.map((page) => page.workspaceSessionKey)).toEqual([
      WORKSPACE_A.sessionKey,
      WORKSPACE_B.sessionKey
    ])
  })
})

describe('right sidebar module availability', () => {
  const reviewModule = getModule('git-review')

  it('treats a capability from the previous workspace as checking', () => {
    expect(
      resolveRightSidebarModuleAvailability(
        reviewModule,
        {
          'git-repository': {
            contextKey: WORKSPACE_A.sessionKey,
            identity: 'repository-a',
            status: 'available'
          }
        },
        WORKSPACE_B
      )
    ).toBe('checking')
  })

  it('makes required capabilities unavailable outside a workspace', () => {
    expect(resolveRightSidebarModuleAvailability(reviewModule, {}, HOME)).toBe('unavailable')
  })

  it('leaves modules without capabilities available in every context', () => {
    expect(resolveRightSidebarModuleAvailability(getModule('browser'), {}, HOME)).toBe('available')
    expect(resolveRightSidebarModuleAvailability(getModule('terminal'), {}, WORKSPACE_B)).toBe(
      'available'
    )
  })

  it('hides workspace-only modules when no project is selected', () => {
    const filesModule = {
      ...createModule('files', 'follow-workspace', 'multiple', 'close-page'),
      requiresWorkspace: true
    }

    expect(resolveRightSidebarModuleAvailability(filesModule, {}, HOME)).toBe('unavailable')
    expect(resolveRightSidebarModuleAvailability(filesModule, {}, WORKSPACE_A)).toBe('available')
  })
})

function createModule(
  id: RightSidebarModuleId,
  contextBinding: RightSidebarModuleDefinition['contextBinding'],
  instancePolicy: RightSidebarModuleDefinition['instancePolicy'],
  unavailablePagePolicy: RightSidebarModuleDefinition['unavailablePagePolicy']
): RightSidebarModuleDefinition {
  return {
    contextBinding,
    createPage: ({ pageId, workspace }) => ({
      id: pageId,
      moduleId: id,
      title: id,
      workspaceKey: contextBinding === 'global' ? null : workspace.key,
      workspacePath: contextBinding === 'global' ? undefined : workspace.path
    }),
    icon: TestIcon,
    id,
    instancePolicy,
    render: () => null,
    retention: 'keep-alive',
    surfaceKind: 'react',
    titleKey: 'rightSidebar.tools',
    unavailablePagePolicy
  }
}

function getModule(id: RightSidebarModuleId): RightSidebarModuleDefinition {
  const module = MODULES.find((candidate) => candidate.id === id)
  if (!module) throw new Error(`Missing test module: ${id}`)
  return module
}

function openPages(
  workspace: RightSidebarWorkspaceContext,
  moduleIds: RightSidebarModuleId[]
): RightSidebarPlatformState {
  return moduleIds.reduce<RightSidebarPlatformState>(
    (state, moduleId) =>
      reduceRightSidebarPlatform(state, {
        module: getModule(moduleId),
        pageId: `${moduleId}-page`,
        t: translate,
        type: 'open',
        workspace
      }),
    INITIAL_RIGHT_SIDEBAR_PLATFORM_STATE
  )
}

function synchronize(
  state: RightSidebarPlatformState,
  workspace: RightSidebarWorkspaceContext,
  reviewStatus: 'checking' | 'available' | 'unavailable',
  workspaceKeys?: readonly string[]
): RightSidebarPlatformState {
  const availability = resolveRightSidebarModuleAvailabilityMap(
    MODULES,
    {
      'git-repository': {
        contextKey: workspace.sessionKey,
        identity: reviewStatus === 'available' ? `repository:${workspace.key}` : undefined,
        status: reviewStatus
      }
    },
    workspace
  )
  return reduceRightSidebarPlatform(state, {
    availability,
    modules: MODULES,
    type: 'synchronize-context',
    workspace,
    workspaceKeys
  })
}

function openFilePage(state: RightSidebarPlatformState, path: string): RightSidebarPlatformState {
  return reduceRightSidebarPlatform(state, {
    module: getModule('files'),
    pageId: `file-${path}`,
    request: {
      moduleState: { kind: 'workspace-file', path },
      resourceKey: `workspace-file:${path}`,
      title: path
    },
    sourcePageId: 'files-page',
    type: 'open-related-page'
  })
}
