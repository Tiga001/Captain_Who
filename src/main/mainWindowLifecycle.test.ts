// Electron main process tests: verify close-to-hide and Dock restoration semantics.

import { describe, expect, it, vi } from 'vitest'
import {
  shouldHideMainWindowOnClose,
  showExistingMainWindow,
  type RestorableMainWindow
} from './mainWindowLifecycle'

function createWindow(options: { destroyed?: boolean; minimized?: boolean } = {}): {
  handle: RestorableMainWindow
  focus: ReturnType<typeof vi.fn>
  restore: ReturnType<typeof vi.fn>
  show: ReturnType<typeof vi.fn>
} {
  const focus = vi.fn()
  const restore = vi.fn()
  const show = vi.fn()
  return {
    focus,
    handle: {
      focus,
      isDestroyed: () => options.destroyed ?? false,
      isMinimized: () => options.minimized ?? false,
      restore,
      show
    },
    restore,
    show
  }
}

describe('main window lifecycle', () => {
  it('hides ordinary macOS closes but allows real quits and other platforms to close', () => {
    expect(shouldHideMainWindowOnClose('darwin', false)).toBe(true)
    expect(shouldHideMainWindowOnClose('darwin', true)).toBe(false)
    expect(shouldHideMainWindowOnClose('win32', false)).toBe(false)
    expect(shouldHideMainWindowOnClose('linux', false)).toBe(false)
  })

  it('shows and focuses the same live window', () => {
    const window = createWindow()

    expect(showExistingMainWindow(window.handle)).toBe(window.handle)
    expect(window.restore).not.toHaveBeenCalled()
    expect(window.show).toHaveBeenCalledOnce()
    expect(window.focus).toHaveBeenCalledOnce()
  })

  it('restores a minimized window before showing it', () => {
    const window = createWindow({ minimized: true })

    expect(showExistingMainWindow(window.handle)).toBe(window.handle)
    expect(window.restore).toHaveBeenCalledOnce()
    expect(window.show).toHaveBeenCalledOnce()
    expect(window.focus).toHaveBeenCalledOnce()
  })

  it('requests a replacement for missing or destroyed windows', () => {
    const destroyedWindow = createWindow({ destroyed: true })

    expect(showExistingMainWindow(null)).toBeNull()
    expect(showExistingMainWindow(destroyedWindow.handle)).toBeNull()
    expect(destroyedWindow.show).not.toHaveBeenCalled()
    expect(destroyedWindow.focus).not.toHaveBeenCalled()
  })
})
