import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import '../../../styles/global.css'
import '../FilesPanel.css'

const { copyPathSpy, listDirectorySpy, openFileSpy, readMetadataSpy, readTextSpy } = vi.hoisted(
  () => ({
    copyPathSpy: vi.fn(),
    listDirectorySpy: vi.fn(),
    openFileSpy: vi.fn(),
    readMetadataSpy: vi.fn(),
    readTextSpy: vi.fn()
  })
)

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))

vi.mock('../filesClient', () => ({
  copyWorkspaceFilePath: copyPathSpy,
  listWorkspaceDirectory: listDirectorySpy,
  readWorkspaceFileMetadata: readMetadataSpy,
  readWorkspaceImageFile: vi.fn(),
  readWorkspaceTextFile: readTextSpy,
  revealWorkspaceFile: vi.fn()
}))

vi.mock('../../gitReview/syntaxHighlighting/useGitReviewSyntaxHighlight', () => ({
  useGitReviewSyntaxHighlight: () => ({
    result: { cacheKey: 'test', language: 'text', lines: [], mode: 'plain' },
    status: 'ready'
  })
}))

const { FilesPanel } = await import('../FilesPanel')

beforeEach(() => {
  copyPathSpy.mockReset()
  copyPathSpy.mockResolvedValue(undefined)
  listDirectorySpy.mockReset()
  openFileSpy.mockReset()
  readMetadataSpy.mockReset()
  readTextSpy.mockReset()

  listDirectorySpy.mockImplementation(async ({ directoryPath = '' }: { directoryPath?: string }) =>
    directoryPath === 'src'
      ? {
          directoryPath,
          entries: [{ kind: 'file', name: 'index.ts', path: 'src/index.ts' }],
          truncated: false
        }
      : {
          directoryPath: '',
          entries: [
            { kind: 'directory', name: 'src', path: 'src/' },
            { kind: 'file', name: 'README.md', path: 'README.md' }
          ],
          truncated: false
        }
  )
  readMetadataSpy.mockResolvedValue({
    kind: 'file',
    mimeType: 'text/plain; charset=utf-8',
    modifiedAtMs: 1,
    path: 'README.md',
    previewKind: 'text',
    sizeBytes: 16
  })
  readTextSpy.mockResolvedValue({
    content: '# Workspace\nHello',
    modifiedAtMs: 1,
    path: 'README.md',
    sizeBytes: 16
  })
})

describe('FilesPanel', () => {
  it('keeps the current preview stable while opening tree selections as new pages', async () => {
    const screen = await render(
      <div style={{ height: 620, width: 620 }}>
        <FilesPanel
          filePath="README.md"
          isActive
          onOpenFile={openFileSpy}
          onSurfaceFocus={() => undefined}
          projectId="project-1"
          projectName="Workspace"
        />
      </div>
    )

    const tree = await expect
      .poll(() => screen.container.querySelector('file-tree-container'))
      .not.toBeNull()
    void tree
    const host = screen.container.querySelector<HTMLElement>('file-tree-container')
    if (!host?.shadowRoot) throw new Error('Pierre tree did not attach an open shadow root')

    const srcRow = await expect
      .poll(() => host.shadowRoot?.querySelector<HTMLButtonElement>('[data-item-path="src/"]'))
      .not.toBeNull()
    void srcRow
    host.shadowRoot.querySelector<HTMLButtonElement>('[data-item-path="src/"]')?.click()

    await expect
      .poll(() => listDirectorySpy.mock.calls.some(([input]) => input.directoryPath === 'src'))
      .toBe(true)
    await expect
      .poll(() => host.shadowRoot?.querySelector('[data-item-path="src/index.ts"]'))
      .not.toBeNull()
    expect(host.shadowRoot.querySelector('[data-file-tree-sticky-overlay="true"]')).not.toBeNull()

    const workspace = screen.container.querySelector<HTMLElement>('.files-panel__workspace')
    const preview = screen.container.querySelector<HTMLElement>('.files-panel__preview')
    const treePane = screen.container.querySelector<HTMLElement>('.files-panel__tree-pane')
    const filterInput = screen.container.querySelector<HTMLInputElement>(
      '.files-panel__tree-pane .files-panel__tree-search input'
    )
    if (!workspace || !preview || !treePane || !filterInput) {
      throw new Error('Files split layout did not render')
    }
    expect(
      Math.abs(preview.getBoundingClientRect().width - treePane.getBoundingClientRect().width)
    ).toBeLessThan(1)

    host.shadowRoot.querySelector<HTMLButtonElement>('[data-item-path="src/index.ts"]')?.click()
    await expect.poll(() => openFileSpy).toHaveBeenCalledWith('src/index.ts')
    await expect
      .poll(() =>
        host.shadowRoot
          ?.querySelector('[data-item-path="README.md"]')
          ?.hasAttribute('data-item-selected')
      )
      .toBe(true)

    await expect.poll(() => readMetadataSpy.mock.calls.length).toBe(1)
    await expect.poll(() => readTextSpy.mock.calls.length).toBe(1)
    await expect
      .poll(() => screen.container.querySelector('.files-panel__code')?.textContent)
      .toContain('# Workspace')

    const breadcrumbPrefix = screen.container.querySelector<HTMLElement>(
      '.files-panel__breadcrumb-prefix'
    )
    const breadcrumbSeparator = screen.container.querySelector<HTMLElement>(
      '.files-panel__breadcrumb-separator'
    )
    const breadcrumbFilename = screen.container.querySelector<HTMLElement>(
      '.files-panel__breadcrumb-filename'
    )
    expect(breadcrumbPrefix?.textContent).toBe('Workspace')
    expect(breadcrumbFilename?.textContent).toBe('README.md')
    expect(breadcrumbPrefix && getComputedStyle(breadcrumbPrefix).direction).toBe('rtl')
    expect(
      breadcrumbPrefix &&
        breadcrumbSeparator &&
        breadcrumbSeparator.getBoundingClientRect().left -
          breadcrumbPrefix.getBoundingClientRect().right
    ).toBeLessThanOrEqual(6)
    expect(
      screen.container.querySelector<HTMLElement>('.files-panel__toolbar')?.getBoundingClientRect()
        .height
    ).toBe(44)

    await screen.getByRole('button', { name: 'files.options' }).click()
    await screen.getByRole('menuitemcheckbox', { name: 'files.wrapLines' }).click()
    expect(
      screen.container.querySelector('.files-panel__code')?.getAttribute('data-wrap-lines')
    ).toBe('true')

    await screen.getByRole('button', { name: 'files.options' }).click()
    await screen.getByRole('menuitem', { name: 'files.copyPath' }).click()
    expect(copyPathSpy).toHaveBeenCalledWith({ path: 'README.md', projectId: 'project-1' })

    await screen.getByRole('button', { name: 'files.hideTree' }).click()
    expect(treePane.isConnected).toBe(true)
    expect(treePane.getAttribute('aria-hidden')).toBe('true')
    await expect
      .poll(() =>
        Math.abs(preview.getBoundingClientRect().width - workspace.getBoundingClientRect().width)
      )
      .toBeLessThan(1)
  })

  it('stacks sticky directory ancestors and replaces only the matching hierarchy level', async () => {
    const createFiles = (directoryPath: string, count: number) =>
      Array.from({ length: count }, (_, index) => {
        const name = `file-${String(index).padStart(2, '0')}.ts`
        return { kind: 'file' as const, name, path: `${directoryPath}/${name}` }
      })

    listDirectorySpy.mockImplementation(
      async ({ directoryPath = '' }: { directoryPath?: string }) => {
        if (directoryPath === 'alpha/branch/leaf') {
          return {
            directoryPath,
            entries: createFiles(directoryPath, 24),
            truncated: false
          }
        }
        if (directoryPath === 'alpha/branch') {
          return {
            directoryPath,
            entries: [
              { kind: 'directory', name: 'leaf', path: 'alpha/branch/leaf/' },
              ...createFiles(directoryPath, 4)
            ],
            truncated: false
          }
        }
        if (directoryPath === 'alpha') {
          return {
            directoryPath,
            entries: [
              { kind: 'directory', name: 'branch', path: 'alpha/branch/' },
              ...createFiles(directoryPath, 4)
            ],
            truncated: false
          }
        }
        if (directoryPath === 'omega') {
          return {
            directoryPath,
            entries: createFiles(directoryPath, 24),
            truncated: false
          }
        }
        return {
          directoryPath: '',
          entries: [
            { kind: 'directory', name: 'alpha', path: 'alpha/' },
            { kind: 'directory', name: 'omega', path: 'omega/' }
          ],
          truncated: false
        }
      }
    )
    readMetadataSpy.mockImplementation(async ({ path }: { path: string }) => ({
      kind: 'file',
      mimeType: 'text/plain; charset=utf-8',
      modifiedAtMs: 1,
      path,
      previewKind: 'text',
      sizeBytes: 16
    }))
    readTextSpy.mockImplementation(async ({ path }: { path: string }) => ({
      content: path,
      modifiedAtMs: 1,
      path,
      sizeBytes: 16
    }))

    const screen = await render(
      <div style={{ height: 360, width: 620 }}>
        <FilesPanel
          filePath="alpha/branch/leaf/file-20.ts"
          isActive
          onOpenFile={openFileSpy}
          onSurfaceFocus={() => undefined}
          projectId="project-1"
          projectName="Workspace"
        />
      </div>
    )

    await expect
      .poll(() => screen.container.querySelector<HTMLElement>('file-tree-container'))
      .not.toBeNull()
    const host = screen.container.querySelector<HTMLElement>('file-tree-container')
    if (!host?.shadowRoot) throw new Error('Pierre tree did not attach an open shadow root')

    const stickyRows = (): HTMLElement[] =>
      Array.from(
        host.shadowRoot?.querySelectorAll<HTMLElement>('[data-file-tree-sticky-row="true"]') ?? []
      )
    const stickyPaths = (): string[] =>
      stickyRows().map((row) => row.dataset.fileTreeStickyPath ?? '')

    await expect.poll(stickyPaths).toEqual(['alpha/', 'alpha/branch/', 'alpha/branch/leaf/'])
    expect(stickyRows().map((row) => row.style.top)).toEqual(['0px', '28px', '56px'])

    const stickyContent = host.shadowRoot.querySelector<HTMLElement>(
      '[data-file-tree-sticky-overlay-content="true"]'
    )
    expect(stickyContent).not.toBeNull()
    expect(stickyContent && getComputedStyle(stickyContent).backgroundColor).not.toBe(
      'rgba(0, 0, 0, 0)'
    )

    const scrollElement = host.shadowRoot.querySelector<HTMLElement>(
      '[data-file-tree-virtualized-scroll="true"]'
    )
    if (!scrollElement) throw new Error('Pierre tree did not render its scroll container')

    scrollElement.scrollTop = scrollElement.scrollHeight
    scrollElement.dispatchEvent(new Event('scroll'))
    await expect
      .poll(() => host.shadowRoot?.querySelector<HTMLButtonElement>('[data-item-path="omega/"]'))
      .not.toBeNull()
    const collapsedTreeHeight = scrollElement.scrollHeight
    host.shadowRoot.querySelector<HTMLButtonElement>('[data-item-path="omega/"]')?.click()

    await expect
      .poll(() => listDirectorySpy.mock.calls.some(([input]) => input.directoryPath === 'omega'))
      .toBe(true)
    await expect.poll(() => scrollElement.scrollHeight).toBeGreaterThan(collapsedTreeHeight)
    scrollElement.scrollTop = scrollElement.scrollHeight
    scrollElement.dispatchEvent(new Event('scroll'))

    await expect.poll(stickyPaths).toEqual(['omega/'])
  })
})
