import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type {
  GitRepositoryInspection,
  GitReviewSummary,
  GitReviewSummaryInput
} from '@mycopilot/protocol'
import type { AppProject } from '../../../config/projectConfig'
import { ToastProvider } from '../../../components/toast/ToastProvider'
import '../../../styles/global.css'

const spies = vi.hoisted(() => ({
  inspect: vi.fn(),
  summary: vi.fn(),
  copy: vi.fn(),
  mutate: vi.fn()
}))
vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'en-US',
    resolvedThemeId: 'classic-light',
    t: (key: string) => key
  })
}))
vi.mock('../gitReviewClient', () => ({
  inspectGitRepository: spies.inspect,
  getGitReviewSummary: spies.summary,
  copyGitReviewFilePath: spies.copy,
  mutateGitReviewFile: spies.mutate,
  getGitReviewRepositoryContext: vi
    .fn()
    .mockResolvedValue({ repositoryId: 'repo', branches: [], truncated: false }),
  listGitReviewCommits: vi
    .fn()
    .mockResolvedValue({ repositoryId: 'repo', commits: [], truncated: false }),
  getGitReviewFileDiff: vi.fn(),
  getGitReviewFileContent: vi.fn()
}))
const { GitReviewPanel } = await import('../GitReviewPanel')
const project: AppProject = {
  id: 'project',
  name: 'Project',
  createdAt: 1,
  folders: [
    { id: 'app', alias: 'app', path: '/project/app', role: 'primary', sortOrder: 0, createdAt: 1 },
    {
      id: 'docs',
      alias: 'docs',
      path: '/project/docs',
      role: 'auxiliary',
      sortOrder: 1,
      createdAt: 1
    }
  ]
}
function inspection(onlyAuxiliary = false): GitRepositoryInspection {
  return {
    projectId: project.id,
    state: 'ready',
    repositoryId: 'repo',
    defaultFolderId: onlyAuxiliary ? 'docs' : 'app',
    folders: project.folders.map((folder) => ({
      folderId: folder.id,
      alias: folder.alias,
      role: folder.role,
      state: onlyAuxiliary && folder.id === 'app' ? 'notRepository' : 'ready',
      repositoryId: `repo-${folder.id}`
    }))
  }
}
function summary(input: GitReviewSummaryInput): GitReviewSummary {
  const sources =
    input.source?.kind === 'all'
      ? project.folders
      : project.folders.filter(
          (folder) =>
            folder.id === (input.source?.kind === 'folder' ? input.source.folderId : 'app')
        )
  const files = sources.map((folder) => ({
    id: `${folder.id}-readme`,
    sourceFolderId: folder.id,
    sourceAlias: folder.alias,
    workspacePath: folder.id === 'app' ? 'README.md' : '@workspace/docs/README.md',
    path: 'README.md',
    status: 'modified' as const,
    stats: { additions: 1, deletions: 0 }
  }))
  return {
    repositoryId: 'repo',
    snapshotId: `snapshot-${input.source?.kind === 'all' ? 'all' : sources[0]?.id}-${input.target.kind}`,
    source: input.source,
    target: input.target,
    context: {},
    files,
    truncated: false,
    ...(input.target.kind === 'lastTurn' ? { assistantMessageId: 'original-reply' } : {}),
    stats: {
      additions: files.length,
      deletions: 0,
      fileCount: files.length,
      lineCountsComplete: true
    }
  }
}
function pane(sourceProject = project, onOpenFile = vi.fn()) {
  return (
    <ToastProvider>
      <div style={{ width: 850, height: 620 }}>
        <GitReviewPanel
          project={sourceProject}
          projectId={sourceProject.id}
          conversationId="conversation"
          isActive
          onOpenFile={onOpenFile}
        />
      </div>
    </ToastProvider>
  )
}
beforeEach(() => {
  vi.clearAllMocks()
  spies.inspect.mockResolvedValue(inspection())
  spies.summary.mockImplementation(async (input: GitReviewSummaryInput) => summary(input))
  spies.copy.mockResolvedValue(undefined)
  window.localStorage.removeItem('mycopilot.gitReview.preferences.v1')
})

describe('multi-folder review', () => {
  it('keeps the single-folder toolbar unchanged but shows the selector with one Git source among multiple folders', async () => {
    const screen = await render(pane({ ...project, folders: [project.folders[0]] }))
    expect(screen.container.querySelector('.git-review__repository-control')).toBeNull()
    expect(spies.inspect).not.toHaveBeenCalled()
    spies.inspect.mockResolvedValue(inspection(true))
    await screen.rerender(pane())
    await expect
      .poll(() => screen.container.querySelector('.git-review__repository-control')?.textContent)
      .toBe('docs')
    await screen.getByRole('button', { name: 'gitReview.repositories.select', exact: true }).click()
    await expect
      .element(screen.getByRole('menuitemradio', { name: 'docs', exact: true }))
      .toBeVisible()
    expect(
      document.querySelector('[role="menuitemradio"][aria-checked="true"]')?.textContent
    ).toContain('docs')
    expect(document.body.textContent).not.toContain('gitReview.repositories.all')
  })

  it('aggregates only last turn, preserves each same-name file origin, and returns to a single repository for working changes', async () => {
    const open = vi.fn()
    const screen = await render(pane(project, open))
    await expect.poll(() => spies.summary.mock.calls.length).toBeGreaterThan(0)
    await screen.getByRole('button', { name: 'gitReview.scope.uncommitted', exact: true }).click()
    await screen
      .getByRole('menuitemradio', { name: 'gitReview.scope.lastTurn', exact: true })
      .click()
    await expect
      .poll(() => screen.container.querySelectorAll('.git-review__repository-heading').length)
      .toBe(2)
    expect(screen.container.querySelector('.git-review__repository-control')?.textContent).toBe(
      'gitReview.repositories.all'
    )
    const docsCard = screen.container.querySelector('[data-file-id="docs-readme"]')!
    ;(docsCard.querySelector('[aria-label="gitReview.file.open"]') as HTMLButtonElement).click()
    expect(open).toHaveBeenLastCalledWith('README.md', 'docs', 'original-reply')
    ;(docsCard.querySelector('[aria-label="gitReview.file.copyPath"]') as HTMLButtonElement).click()
    await expect.poll(() => spies.copy.mock.calls.length).toBe(1)
    expect(spies.copy).toHaveBeenLastCalledWith({
      projectId: 'project',
      path: 'README.md',
      folderId: 'docs',
      assistantMessageId: 'original-reply'
    })
    await screen.getByRole('button', { name: 'gitReview.scope.lastTurn', exact: true }).click()
    await screen
      .getByRole('menuitemradio', { name: 'gitReview.scope.unstaged', exact: true })
      .click()
    await expect
      .poll(() => spies.summary.mock.calls.at(-1)?.[0])
      .toMatchObject({ target: { kind: 'unstaged' }, source: { kind: 'folder', folderId: 'app' } })
    expect(
      spies.summary.mock.calls.some(
        ([input]) => input.source?.kind === 'all' && input.target.kind !== 'lastTurn'
      )
    ).toBe(false)
    await screen.getByRole('button', { name: 'gitReview.repositories.select', exact: true }).click()
    expect(document.querySelector('[role="menu"]')?.textContent).not.toContain(
      'gitReview.repositories.all'
    )
  })

  it('ignores a late summary from the previous source folder', async () => {
    let release!: (value: GitReviewSummary) => void
    const slow = new Promise<GitReviewSummary>((resolve) => {
      release = resolve
    })
    spies.summary.mockImplementation((input: GitReviewSummaryInput) =>
      input.source?.kind === 'folder' && input.source.folderId === 'app'
        ? slow
        : Promise.resolve(summary(input))
    )
    const screen = await render(pane())
    await expect.poll(() => spies.summary.mock.calls.length).toBe(1)
    await screen.getByRole('button', { name: 'gitReview.repositories.select', exact: true }).click()
    await screen.getByRole('menuitemradio', { name: 'docs', exact: true }).click()
    await expect
      .poll(() => screen.container.querySelector('[data-file-id="docs-readme"]') !== null)
      .toBe(true)
    release(
      summary({
        projectId: 'project',
        target: { kind: 'uncommitted' },
        source: { kind: 'folder', folderId: 'app' }
      })
    )
    await Promise.resolve()
    expect(screen.container.querySelector('[data-file-id="app-readme"]')).toBeNull()
    expect(screen.container.querySelector('[data-file-id="docs-readme"]')).not.toBeNull()
  })
})
