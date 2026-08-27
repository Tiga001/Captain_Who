import { afterEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { BROWSER_DOWNLOAD_SCHEMA_VERSION } from '@mycopilot/protocol'

const perform = vi.fn(async () => 'performed' as const)
const openDirectory = vi.fn(async () => 'opened' as const)

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ language: 'zh-CN', t: (key: string) => key })
}))

vi.mock('../../browser/useBrowserDownloadCenter', () => ({
  useBrowserDownloadCenter: () => ({
    openDirectory,
    perform,
    snapshot: {
      schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
      revision: 3,
      downloads: [
        {
          downloadId: 'browser-download:123e4567-e89b-42d3-a456-426614174000',
          displayName: 'large-archive.zip',
          source: 'manual',
          state: 'progressing',
          receivedBytes: 25,
          totalBytes: 100,
          bytesPerSecond: 10,
          startedAt: 1_000,
          updatedAt: 2_000,
          canPause: true,
          canResume: false,
          canCancel: true,
          canReveal: false,
          canCopyUrl: true,
          canCopyPath: false
        },
        {
          downloadId: 'browser-download:223e4567-e89b-42d3-a456-426614174000',
          displayName: 'stopped.zip',
          source: 'manual',
          state: 'cancelled',
          receivedBytes: 20,
          totalBytes: 100,
          bytesPerSecond: 0,
          startedAt: 1_000,
          updatedAt: 2_000,
          canPause: false,
          canResume: false,
          canCancel: false,
          canReveal: false,
          canCopyUrl: true,
          canCopyPath: false
        },
        {
          downloadId: 'browser-download:323e4567-e89b-42d3-a456-426614174000',
          displayName: 'paused.zip',
          source: 'manual',
          state: 'paused',
          receivedBytes: 40,
          totalBytes: 100,
          bytesPerSecond: 0,
          startedAt: 1_000,
          updatedAt: 2_000,
          canPause: false,
          canResume: true,
          canCancel: true,
          canReveal: false,
          canCopyUrl: true,
          canCopyPath: false
        }
      ]
    }
  })
}))

const { BrowserDownloadCenter } = await import('../../browser/BrowserDownloadCenter')

afterEach(() => {
  perform.mockClear()
  openDirectory.mockClear()
})

describe('BrowserDownloadCenter', () => {
  it('renders live progress and dispatches real transfer controls', async () => {
    const onOpenChange = vi.fn()
    const screen = await render(<BrowserDownloadCenter isOpen onOpenChange={onOpenChange} />)

    await expect.element(screen.getByRole('dialog')).toBeVisible()
    const dialog = document.body.querySelector('.browser-download-center__panel')
    expect(dialog?.parentElement).toBe(document.body)
    expect((dialog as HTMLElement | null)?.style.width).not.toBe('')
    await expect
      .element(screen.getByRole('progressbar', { name: '25%' }))
      .toHaveAttribute('aria-valuenow', '25')
    await screen.getByRole('button', { name: 'browser.downloadCenter.pause' }).click()
    expect(perform).toHaveBeenCalledWith(
      'browser-download:123e4567-e89b-42d3-a456-426614174000',
      'pause'
    )
    await screen.getByRole('button', { name: 'browser.downloadCenter.stop' }).nth(0).click()
    expect(perform).toHaveBeenCalledWith(
      'browser-download:123e4567-e89b-42d3-a456-426614174000',
      'cancel'
    )
    await screen.getByRole('button', { name: 'browser.downloadCenter.openFolder' }).click()
    expect(openDirectory).toHaveBeenCalledTimes(1)
    expect(onOpenChange).not.toHaveBeenCalled()
  })

  it('keeps stopped downloads actionable through the overflow menu', async () => {
    const screen = await render(<BrowserDownloadCenter isOpen onOpenChange={vi.fn()} />)
    const overflowButtons = screen.getByRole('button', {
      name: 'browser.downloadCenter.moreActions'
    })
    await overflowButtons.nth(0).click()
    await screen.getByRole('menuitem', { name: 'browser.downloadCenter.copyUrl' }).click()
    expect(perform).toHaveBeenCalledWith(
      'browser-download:223e4567-e89b-42d3-a456-426614174000',
      'copy_url'
    )
  })

  it('shows inverse portaled tooltips for the described download actions', async () => {
    const screen = await render(<BrowserDownloadCenter isOpen onOpenChange={vi.fn()} />)
    const actions = [
      [
        'browser.downloadCenter.title',
        screen.getByRole('button', { name: 'browser.downloadCenter.title' })
      ],
      [
        'browser.downloadCenter.openFolder',
        screen.getByRole('button', { name: 'browser.downloadCenter.openFolder' })
      ],
      [
        'browser.downloadCenter.pause',
        screen.getByRole('button', { name: 'browser.downloadCenter.pause' })
      ],
      [
        'browser.downloadCenter.resume',
        screen.getByRole('button', { name: 'browser.downloadCenter.resume' })
      ],
      [
        'browser.downloadCenter.stop',
        screen.getByRole('button', { name: 'browser.downloadCenter.stop' }).nth(0)
      ]
    ] as const

    for (const [label, action] of actions) {
      ;(action.element() as HTMLButtonElement).focus()
      const tooltip = screen.getByRole('tooltip')
      await expect.element(tooltip).toHaveTextContent(label)
      await expect.element(tooltip).toHaveAttribute('data-appearance', 'inverse')
      expect(tooltip.element().parentElement).toBe(document.body)
      expect(action.element().hasAttribute('title')).toBe(false)
    }
  })

  it('waits one second before showing hover help and omits it from overflow buttons', async () => {
    const screen = await render(<BrowserDownloadCenter isOpen onOpenChange={vi.fn()} />)
    const folderButton = screen.getByRole('button', {
      name: 'browser.downloadCenter.openFolder'
    })

    await folderButton.hover()
    await new Promise((resolve) => window.setTimeout(resolve, 900))
    expect(document.querySelector('.mc-tooltip')).toBeNull()
    await new Promise((resolve) => window.setTimeout(resolve, 150))
    await expect
      .element(screen.getByRole('tooltip'))
      .toHaveTextContent('browser.downloadCenter.openFolder')

    await screen.getByRole('button', { name: 'browser.downloadCenter.moreActions' }).hover()
    await new Promise((resolve) => window.setTimeout(resolve, 1_050))
    expect(document.querySelector('.mc-tooltip')).toBeNull()
  })
})
