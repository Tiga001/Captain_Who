import { useState } from 'react'
import type { ComponentProps } from 'react'
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
  openWorkspaceExternalLink: vi.fn(),
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

const [{ FilesPanel }, { WorkspaceFileTreeSessionsProvider }] = await Promise.all([
  import('../FilesPanel'),
  import('../WorkspaceFileTreeSessions')
])

type TestFilesPanelProps = Omit<
  ComponentProps<typeof FilesPanel>,
  'markdownView' | 'onMarkdownViewChange' | 'onWrapLinesChange' | 'wrapLines'
>

function TestFilesPanel(props: TestFilesPanelProps) {
  const [markdownView, setMarkdownView] = useState<'preview' | 'source'>('source')
  const [wrapLines, setWrapLines] = useState(false)
  return (
    <WorkspaceFileTreeSessionsProvider projectIds={[props.projectId]}>
      <FilesPanel
        {...props}
        markdownView={markdownView}
        onMarkdownViewChange={setMarkdownView}
        onWrapLinesChange={setWrapLines}
        wrapLines={wrapLines}
      />
    </WorkspaceFileTreeSessionsProvider>
  )
}

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
  it('renders Office files as a dedicated unsupported preview without reading file contents', async () => {
    const screen = await render(
      <div style={{ height: 620, width: 620 }}>
        <TestFilesPanel
          filePath="reports/quarterly.docx"
          isActive
          onOpenFile={openFileSpy}
          onSurfaceFocus={() => undefined}
          projectId="project-1"
          projectName="Workspace"
        />
      </div>
    )

    const message = screen.container.querySelector<HTMLElement>(
      '.files-panel__center-state[data-variant="unsupported"]'
    )
    expect(message?.textContent).toContain('files.preview.officeTitle')
    expect(message?.textContent).toContain('files.preview.officeDescription')
    expect(message?.querySelector('.files-panel__center-icon')).toBeNull()
    await new Promise<void>((resolve) => window.requestAnimationFrame(() => resolve()))
    expect(readMetadataSpy).not.toHaveBeenCalled()
    expect(readTextSpy).not.toHaveBeenCalled()
  })

  it('switches Markdown files between source and rendered preview modes', async () => {
    readTextSpy.mockResolvedValue({
      content: '# Workspace\n\n| Item | State |\n| --- | --- |\n| Preview | Ready |',
      modifiedAtMs: 1,
      path: 'README.md',
      sizeBytes: 64
    })

    const screen = await render(
      <div style={{ height: 620, width: 620 }}>
        <TestFilesPanel
          filePath="README.md"
          isActive
          onOpenFile={openFileSpy}
          onSurfaceFocus={() => undefined}
          projectId="project-1"
          projectName="Workspace"
        />
      </div>
    )

    await expect
      .poll(() => screen.container.querySelector('.files-panel__code')?.textContent)
      .toContain('# Workspace')

    await screen.getByRole('button', { name: 'files.options' }).click()
    const sourceOption = screen.getByRole('menuitemradio', { name: 'files.markdown.source' })
    const previewOption = screen.getByRole('menuitemradio', { name: 'files.markdown.preview' })
    await expect.element(sourceOption).toHaveAttribute('aria-checked', 'true')
    await expect.element(previewOption).toHaveAttribute('aria-checked', 'false')
    await previewOption.click()

    await expect
      .poll(() => screen.container.querySelector('.files-panel__markdown h1')?.textContent)
      .toBe('Workspace')
    expect(screen.container.querySelector('.files-panel__markdown table')?.textContent).toContain(
      'Preview'
    )
    expect(screen.container.querySelector('.files-panel__code')).toBeNull()

    await screen.getByRole('button', { name: 'files.options' }).click()
    await screen.getByRole('menuitemradio', { name: 'files.markdown.source' }).click()
    await expect
      .poll(() => screen.container.querySelector('.files-panel__code')?.textContent)
      .toContain('# Workspace')
    expect(screen.container.querySelector('.files-panel__markdown')).toBeNull()
    expect(readTextSpy).toHaveBeenCalledTimes(1)
  })

  it('keeps the current preview stable while opening tree selections as new pages', async () => {
    const screen = await render(
      <div style={{ height: 620, width: 620 }}>
        <TestFilesPanel
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
        <TestFilesPanel
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

  it('shares one tree model and directory cache across file-page remounts', async () => {
    function SharedTreeHarness() {
      const [filePath, setFilePath] = useState('README.md')
      const [wrapLines, setWrapLines] = useState(false)

      return (
        <WorkspaceFileTreeSessionsProvider projectIds={['project-1']}>
          <button type="button" onClick={() => setFilePath('src/index.ts')}>
            switch file
          </button>
          <div style={{ height: 620, width: 620 }}>
            <FilesPanel
              filePath={filePath}
              isActive
              key={filePath}
              markdownView="source"
              onMarkdownViewChange={() => undefined}
              onOpenFile={openFileSpy}
              onSurfaceFocus={() => undefined}
              onWrapLinesChange={setWrapLines}
              projectId="project-1"
              projectName="Workspace"
              wrapLines={wrapLines}
            />
          </div>
        </WorkspaceFileTreeSessionsProvider>
      )
    }

    const screen = await render(<SharedTreeHarness />)
    await expect
      .poll(() => screen.container.querySelector<HTMLElement>('file-tree-container'))
      .not.toBeNull()
    const firstHost = screen.container.querySelector<HTMLElement>('file-tree-container')
    if (!firstHost?.shadowRoot) throw new Error('Pierre tree did not attach an open shadow root')

    await expect
      .poll(() => firstHost.shadowRoot?.querySelector<HTMLButtonElement>('[data-item-path="src/"]'))
      .not.toBeNull()
    firstHost.shadowRoot.querySelector<HTMLButtonElement>('[data-item-path="src/"]')?.click()
    await expect
      .poll(() => firstHost.shadowRoot?.querySelector('[data-item-path="src/index.ts"]'))
      .not.toBeNull()
    await screen.getByRole('textbox', { name: 'files.filter' }).fill('index')

    await screen.getByRole('button', { name: 'switch file' }).click()

    await expect.poll(() => firstHost.isConnected).toBe(false)
    await expect
      .poll(() => screen.container.querySelector<HTMLElement>('file-tree-container'))
      .not.toBeNull()
    const remountedHost = screen.container.querySelector<HTMLElement>('file-tree-container')
    if (!remountedHost?.shadowRoot) {
      throw new Error('Shared Pierre tree did not attach to the replacement host')
    }
    await expect
      .poll(() => remountedHost.shadowRoot?.querySelector('[data-item-path="src/index.ts"]'))
      .not.toBeNull()
    await expect
      .poll(
        () =>
          screen.container.querySelector<HTMLInputElement>('.files-panel__tree-search input')?.value
      )
      .toBe('index')
    expect(listDirectorySpy.mock.calls.filter(([input]) => input.directoryPath === '').length).toBe(
      1
    )
    expect(
      listDirectorySpy.mock.calls.filter(([input]) => input.directoryPath === 'src').length
    ).toBe(1)
  })
})
