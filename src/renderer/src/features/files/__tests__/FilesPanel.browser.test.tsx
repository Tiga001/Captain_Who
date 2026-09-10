import { useState } from 'react'
import type { ComponentProps } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import '../../../styles/global.css'
import '../FilesPanel.css'

const {
  copyPathSpy,
  createPdfLoadingTaskSpy,
  getPdfPageSpy,
  listDirectorySpy,
  openExternalSpy,
  openFileSpy,
  pdfDocumentDestroySpy,
  pdfLoadingTaskDestroySpy,
  pdfPageChangeSpy,
  pdfPageCleanupSpy,
  pdfRenderCancelSpy,
  readPreviewSpy
} = vi.hoisted(() => ({
  copyPathSpy: vi.fn(),
  createPdfLoadingTaskSpy: vi.fn(),
  getPdfPageSpy: vi.fn(),
  listDirectorySpy: vi.fn(),
  openExternalSpy: vi.fn(),
  openFileSpy: vi.fn(),
  pdfDocumentDestroySpy: vi.fn(),
  pdfLoadingTaskDestroySpy: vi.fn(),
  pdfPageChangeSpy: vi.fn(),
  pdfPageCleanupSpy: vi.fn(),
  pdfRenderCancelSpy: vi.fn(),
  readPreviewSpy: vi.fn()
}))

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))

vi.mock('../filesClient', () => ({
  copyWorkspaceFilePath: copyPathSpy,
  listWorkspaceDirectory: listDirectorySpy,
  openWorkspaceExternalLink: openExternalSpy,
  readWorkspaceFilePreview: readPreviewSpy,
  revealWorkspaceFile: vi.fn()
}))

vi.mock('../workspacePdfRuntime', () => ({
  createWorkspacePdfLoadingTask: createPdfLoadingTaskSpy
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
  | 'markdownView'
  | 'onMarkdownViewChange'
  | 'onPdfPageChange'
  | 'onWrapLinesChange'
  | 'pdfPage'
  | 'wrapLines'
> & { initialMarkdownView?: 'preview' | 'source' }

function TestFilesPanel({ initialMarkdownView = 'preview', ...props }: TestFilesPanelProps) {
  const [markdownView, setMarkdownView] = useState<'preview' | 'source'>(initialMarkdownView)
  const [pdfPage, setPdfPage] = useState(1)
  const [wrapLines, setWrapLines] = useState(false)
  return (
    <WorkspaceFileTreeSessionsProvider projectIds={[props.projectId]}>
      <FilesPanel
        {...props}
        markdownView={markdownView}
        onMarkdownViewChange={setMarkdownView}
        onPdfPageChange={(page) => {
          pdfPageChangeSpy(page)
          setPdfPage(page)
        }}
        onWrapLinesChange={setWrapLines}
        pdfPage={pdfPage}
        wrapLines={wrapLines}
      />
    </WorkspaceFileTreeSessionsProvider>
  )
}

beforeEach(() => {
  copyPathSpy.mockReset()
  copyPathSpy.mockResolvedValue(undefined)
  listDirectorySpy.mockReset()
  openExternalSpy.mockReset()
  openExternalSpy.mockResolvedValue(undefined)
  openFileSpy.mockReset()
  createPdfLoadingTaskSpy.mockReset()
  getPdfPageSpy.mockReset()
  pdfDocumentDestroySpy.mockReset()
  pdfDocumentDestroySpy.mockResolvedValue(undefined)
  pdfLoadingTaskDestroySpy.mockReset()
  pdfLoadingTaskDestroySpy.mockResolvedValue(undefined)
  pdfPageChangeSpy.mockReset()
  pdfPageCleanupSpy.mockReset()
  pdfRenderCancelSpy.mockReset()
  readPreviewSpy.mockReset()

  getPdfPageSpy.mockImplementation(async (pageNumber: number) => ({
    cleanup: pdfPageCleanupSpy,
    getViewport: ({ scale }: { scale: number }) => ({
      height: 600 * scale,
      width: 400 * scale
    }),
    render: ({ canvas }: { canvas: HTMLCanvasElement }) => {
      const context = canvas.getContext('2d')
      if (!context) throw new Error('Canvas is unavailable in the browser test')
      context.fillStyle = pageNumber === 1 ? '#ff0000' : '#00ff00'
      context.fillRect(0, 0, canvas.width, canvas.height)
      return { cancel: pdfRenderCancelSpy, promise: Promise.resolve() }
    }
  }))
  createPdfLoadingTaskSpy.mockImplementation(async () => ({
    destroy: pdfLoadingTaskDestroySpy,
    promise: Promise.resolve({
      destroy: pdfDocumentDestroySpy,
      getPage: getPdfPageSpy,
      numPages: 3
    })
  }))

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
  readPreviewSpy.mockResolvedValue({
    metadata: {
      kind: 'file',
      mimeType: 'text/plain; charset=utf-8',
      modifiedAtMs: 1,
      path: 'README.md',
      previewKind: 'text',
      sizeBytes: 16
    },
    text: {
      content: '# Workspace\nHello',
      modifiedAtMs: 1,
      path: 'README.md',
      sizeBytes: 16
    }
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
    expect(readPreviewSpy).not.toHaveBeenCalled()
  })

  it('switches Markdown files between source and rendered preview modes', async () => {
    readPreviewSpy.mockResolvedValue({
      metadata: {
        kind: 'file',
        mimeType: 'text/plain; charset=utf-8',
        modifiedAtMs: 1,
        path: 'README.md',
        previewKind: 'text',
        sizeBytes: 64
      },
      text: {
        content: '# Workspace\n\n| Item | State |\n| --- | --- |\n| Preview | Ready |',
        modifiedAtMs: 1,
        path: 'README.md',
        sizeBytes: 64
      }
    })

    const screen = await render(
      <div style={{ height: 620, width: 620 }}>
        <TestFilesPanel
          filePath="README.md"
          initialMarkdownView="source"
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
    const optionsMenu = screen.container.querySelector<HTMLElement>('.files-panel__options-menu')
    expect(getComputedStyle(sourceOption.element()).fontSize).toBe('13px')
    expect(getComputedStyle(sourceOption.element()).lineHeight).toBe('18px')
    expect(optionsMenu && sourceOption.element().scrollWidth <= optionsMenu.clientWidth).toBe(true)
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
    expect(readPreviewSpy).toHaveBeenCalledTimes(1)
  })

  it('renders front matter, rich Markdown, workspace links, and local images', async () => {
    const markdownPath = 'guides/context.md'
    const markdown = `---
status: current
audience:
  - developers
  - maintainers
owner: engineering
metadata:
  version: 2.2.0
---
# Context Management

See [Conversation Trace](../docs/conversation-trace.md#details) and [OpenAI](https://openai.com/).

![Architecture](./assets/architecture.png)

## Responsibilities

- Keep context bounded
- Preserve sources
`
    readPreviewSpy.mockImplementation(async ({ path }: { path: string }) => {
      if (path === 'guides/assets/architecture.png') {
        return {
          image: {
            data: 'iVBORw==',
            mimeType: 'image/png',
            modifiedAtMs: 1,
            path,
            sizeBytes: 4
          },
          metadata: {
            kind: 'file',
            mimeType: 'image/png',
            modifiedAtMs: 1,
            path,
            previewKind: 'image',
            sizeBytes: 4
          }
        }
      }
      return {
        metadata: {
          kind: 'file',
          mimeType: 'text/plain; charset=utf-8',
          modifiedAtMs: 1,
          path: markdownPath,
          previewKind: 'text',
          sizeBytes: markdown.length
        },
        text: {
          content: markdown,
          modifiedAtMs: 1,
          path: markdownPath,
          sizeBytes: markdown.length
        }
      }
    })

    const screen = await render(
      <div style={{ height: 620, width: 620 }}>
        <TestFilesPanel
          filePath={markdownPath}
          isActive
          onOpenFile={openFileSpy}
          onSurfaceFocus={() => undefined}
          projectId="project-1"
          projectName="Workspace"
        />
      </div>
    )

    await expect
      .poll(() => screen.container.querySelector<HTMLElement>('.files-panel__markdown h1'))
      .not.toBeNull()
    const heading = screen.container.querySelector<HTMLElement>('.files-panel__markdown h1')
    expect(heading?.textContent).toBe('Context Management')
    expect(heading?.id).toBe('context-management')

    const metadata = screen.container.querySelector<HTMLElement>('.files-panel__markdown-metadata')
    expect(metadata?.textContent).toContain('files.markdown.metadata')
    expect(metadata?.textContent).toContain('status')
    expect(metadata?.textContent).toContain('current')
    expect(metadata?.textContent).toContain('developers')
    expect(metadata?.textContent).toContain('metadata.version')
    expect(metadata?.textContent).toContain('2.2.0')
    expect(screen.container.querySelector('.files-panel__markdown')?.textContent).not.toContain(
      'status:'
    )

    await screen.getByRole('link', { name: 'Conversation Trace' }).click()
    expect(openFileSpy).toHaveBeenCalledWith('docs/conversation-trace.md', 'details')

    await screen.getByRole('link', { name: 'OpenAI' }).click()
    expect(openExternalSpy).toHaveBeenCalledWith('https://openai.com/')

    await expect
      .poll(() => screen.container.querySelector<HTMLImageElement>('img[alt="Architecture"]'))
      .not.toBeNull()
    const image = screen.container.querySelector<HTMLImageElement>('img[alt="Architecture"]')
    expect(image?.getAttribute('src')).toBe('data:image/png;base64,iVBORw==')
    expect(readPreviewSpy).toHaveBeenCalledWith({
      path: 'guides/assets/architecture.png',
      projectId: 'project-1'
    })

    await screen.getByRole('button', { name: 'files.options' }).click()
    await screen.getByRole('menuitemradio', { name: 'files.markdown.source' }).click()
    await expect
      .poll(() => screen.container.querySelector('.files-panel__code')?.textContent)
      .toContain('status: current')
  })

  it('renders one PDF page at a time and persists pager changes without reloading the document', async () => {
    const pdfData = new Uint8Array([0x25, 0x50, 0x44, 0x46, 0x2d])
    readPreviewSpy.mockResolvedValue({
      metadata: {
        kind: 'file',
        mimeType: 'application/pdf',
        modifiedAtMs: 1,
        path: 'paper.pdf',
        previewKind: 'pdf',
        sizeBytes: pdfData.byteLength
      },
      pdf: {
        data: pdfData,
        mimeType: 'application/pdf',
        modifiedAtMs: 1,
        path: 'paper.pdf',
        sizeBytes: pdfData.byteLength
      }
    })

    const screen = await render(
      <div style={{ height: 620, width: 620 }}>
        <TestFilesPanel
          filePath="paper.pdf"
          isActive
          onOpenFile={openFileSpy}
          onSurfaceFocus={() => undefined}
          projectId="project-1"
          projectName="Workspace"
        />
      </div>
    )

    await expect
      .poll(
        () => screen.container.querySelector<HTMLCanvasElement>('.files-panel__pdf-page')?.width
      )
      .toBeGreaterThan(0)
    expect(createPdfLoadingTaskSpy).toHaveBeenCalledTimes(1)
    expect(createPdfLoadingTaskSpy).toHaveBeenCalledWith(pdfData)
    expect(getPdfPageSpy).toHaveBeenCalledWith(1)

    const canvas = screen.container.querySelector<HTMLCanvasElement>('.files-panel__pdf-page')
    const firstPixel = canvas?.getContext('2d')?.getImageData(0, 0, 1, 1).data
    expect(Array.from(firstPixel ?? [])).toEqual([255, 0, 0, 255])

    const previousPage = screen.getByRole('button', { name: 'files.pdf.previousPage' })
    const nextPage = screen.getByRole('button', { name: 'files.pdf.nextPage' })
    await expect.element(previousPage).toBeDisabled()
    await expect.element(nextPage).toBeEnabled()
    const pager = screen.container.querySelector<HTMLElement>('.files-panel__pdf-pager')
    const preview = screen.container.querySelector<HTMLElement>('.files-panel__preview')
    expect(pager?.textContent).toContain('1/3')
    expect(getComputedStyle(pager!).left).toBe('12px')
    expect(getComputedStyle(pager!).bottom).toBe('12px')
    expect(pager!.getBoundingClientRect().left).toBeLessThan(
      preview!.getBoundingClientRect().left + preview!.clientWidth / 2
    )
    expect(pager!.getBoundingClientRect().top).toBeGreaterThan(
      preview!.getBoundingClientRect().top + preview!.clientHeight / 2
    )

    await nextPage.click()
    await expect.poll(() => pdfPageChangeSpy).toHaveBeenCalledWith(2)
    await expect.poll(() => getPdfPageSpy).toHaveBeenCalledWith(2)
    await expect
      .poll(() => screen.container.querySelector('.files-panel__pdf-pager')?.textContent)
      .toContain('2/3')
    const secondPixel = canvas?.getContext('2d')?.getImageData(0, 0, 1, 1).data
    expect(Array.from(secondPixel ?? [])).toEqual([0, 255, 0, 255])
    expect(createPdfLoadingTaskSpy).toHaveBeenCalledTimes(1)

    screen.unmount()
    await expect.poll(() => pdfDocumentDestroySpy).toHaveBeenCalledTimes(1)
  })

  it('forwards both new selections and repeated clicks on the selected file', async () => {
    const screen = await render(
      <div style={{ height: 620, width: 620 }}>
        <TestFilesPanel
          filePath="README.md"
          initialMarkdownView="source"
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

    await expect
      .poll(() =>
        host.shadowRoot?.querySelector<HTMLButtonElement>(
          '[data-item-path="README.md"][data-item-selected]'
        )
      )
      .not.toBeNull()
    const selectedReadmeRow = host.shadowRoot.querySelector<HTMLButtonElement>(
      '[data-item-path="README.md"][data-item-selected]'
    )
    if (!selectedReadmeRow) throw new Error('Selected README row did not render')
    selectedReadmeRow.dispatchEvent(
      new PointerEvent('pointerdown', { bubbles: true, button: 0, composed: true })
    )
    await expect.poll(() => openFileSpy).toHaveBeenCalledWith('README.md')
    openFileSpy.mockClear()

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

    await expect.poll(() => readPreviewSpy.mock.calls.length).toBe(1)
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
    readPreviewSpy.mockImplementation(async ({ path }: { path: string }) => ({
      metadata: {
        kind: 'file',
        mimeType: 'text/plain; charset=utf-8',
        modifiedAtMs: 1,
        path,
        previewKind: 'text',
        sizeBytes: 16
      },
      text: {
        content: path,
        modifiedAtMs: 1,
        path,
        sizeBytes: 16
      }
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
              onPdfPageChange={() => undefined}
              onSurfaceFocus={() => undefined}
              onWrapLinesChange={setWrapLines}
              pdfPage={1}
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
