import type { GitRepositoryInspection } from '@mycopilot/protocol'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { createRightSidebarWorkspaceSessionKey } from '../../rightSidebar/rightSidebarWorkspace'

const { inspectGitRepositorySpy } = vi.hoisted(() => ({
  inspectGitRepositorySpy: vi.fn()
}))

vi.mock('../gitReviewClient', () => ({
  inspectGitRepository: inspectGitRepositorySpy
}))

const { useGitRepositoryCapability } = await import('../useGitRepositoryCapability')

beforeEach(() => {
  inspectGitRepositorySpy.mockReset()
})

describe('useGitRepositoryCapability', () => {
  it('ignores out-of-order inspection results across rapid workspace switches', async () => {
    const inspectionA = deferred<GitRepositoryInspection>()
    const inspectionB = deferred<GitRepositoryInspection>()
    inspectGitRepositorySpy.mockImplementation((projectId: string) => {
      if (projectId === 'project-a') return inspectionA.promise
      if (projectId === 'project-b') return inspectionB.promise
      throw new Error(`Unexpected project: ${projectId}`)
    })

    const screen = await render(<CapabilityProbe projectId="project-a" workspacePath="/repo/a" />)
    await expect.poll(() => inspectGitRepositorySpy.mock.calls.length).toBe(1)

    await screen.rerender(<CapabilityProbe projectId="project-b" workspacePath="/repo/b" />)
    await expect.poll(() => inspectGitRepositorySpy.mock.calls.length).toBe(2)
    expect(readProbe(screen.container).status).toBe('checking')
    expect(readProbe(screen.container).contextKey).toBe(
      createRightSidebarWorkspaceSessionKey('project-b', '/repo/b')
    )

    inspectionB.resolve({
      projectId: 'project-b',
      repositoryId: 'repository-b',
      folders: [],
      state: 'ready'
    })
    await expect.poll(() => readProbe(screen.container).status).toBe('available')
    expect(readProbe(screen.container).identity).toBe('repository-b')

    inspectionA.resolve({ projectId: 'project-a', state: 'notRepository', folders: [] })
    await Promise.resolve()
    await Promise.resolve()

    expect(readProbe(screen.container)).toEqual({
      contextKey: createRightSidebarWorkspaceSessionKey('project-b', '/repo/b'),
      identity: 'repository-b',
      status: 'available'
    })
  })

  it('does not reinspect another conversation in the same workspace', async () => {
    const inspection = deferred<GitRepositoryInspection>()
    inspectGitRepositorySpy.mockReturnValue(inspection.promise)
    const screen = await render(<CapabilityProbe projectId="project-a" workspacePath="/repo/a" />)
    await expect.poll(() => inspectGitRepositorySpy.mock.calls.length).toBe(1)
    inspection.resolve({
      projectId: 'project-a',
      repositoryId: 'repository-a',
      folders: [],
      state: 'ready'
    })
    await expect.poll(() => readProbe(screen.container).status).toBe('available')

    await screen.rerender(<CapabilityProbe projectId="project-a" workspacePath="/repo/a" />)

    expect(inspectGitRepositorySpy).toHaveBeenCalledTimes(1)
    expect(readProbe(screen.container).identity).toBe('repository-a')
  })

  it('reports unavailable without inspecting when there is no workspace', async () => {
    const screen = await render(<CapabilityProbe projectId={null} workspacePath={undefined} />)

    expect(readProbe(screen.container).status).toBe('unavailable')
    expect(inspectGitRepositorySpy).not.toHaveBeenCalled()
  })

  it('enables review when a Git auxiliary folder is added without changing the primary folder', async () => {
    inspectGitRepositorySpy
      .mockResolvedValueOnce({
        projectId: 'project-a',
        state: 'notRepository',
        folders: []
      })
      .mockResolvedValueOnce({
        projectId: 'project-a',
        state: 'ready',
        repositoryId: 'aux-repository',
        defaultFolderId: 'aux',
        folders: [
          { folderId: 'main', alias: 'main', role: 'primary', state: 'notRepository' },
          {
            folderId: 'aux',
            alias: 'aux',
            role: 'auxiliary',
            state: 'ready',
            repositoryId: 'aux-repository'
          }
        ]
      })
    const screen = await render(
      <CapabilityProbe projectId="project-a" workspacePath="/plain" revision="one" />
    )
    await expect.poll(() => readProbe(screen.container).status).toBe('unavailable')
    await screen.rerender(
      <CapabilityProbe projectId="project-a" workspacePath="/plain" revision="two" />
    )
    await expect.poll(() => readProbe(screen.container).status).toBe('available')
    expect(readProbe(screen.container).identity).toBe('aux-repository')
    expect(inspectGitRepositorySpy).toHaveBeenCalledTimes(2)
  })
})

function CapabilityProbe({
  projectId,
  workspacePath,
  revision
}: {
  projectId: string | null
  workspacePath: string | undefined
  revision?: string
}) {
  const capability = useGitRepositoryCapability(projectId, workspacePath, revision)
  return (
    <output
      data-testid="capability"
      data-context-key={capability.contextKey}
      data-identity={capability.identity}
      data-status={capability.status}
    />
  )
}

function readProbe(container: HTMLElement) {
  const probe = container.querySelector<HTMLOutputElement>('[data-testid="capability"]')
  if (!probe) throw new Error('Missing capability probe')
  return {
    contextKey: probe.dataset.contextKey,
    identity: probe.dataset.identity,
    status: probe.dataset.status
  }
}

function deferred<T>() {
  let resolvePromise: (value: T) => void = () => undefined
  const promise = new Promise<T>((resolve) => {
    resolvePromise = resolve
  })
  return { promise, resolve: resolvePromise }
}
