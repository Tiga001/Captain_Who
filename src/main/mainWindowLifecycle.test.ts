// Electron main process tests: verify close-to-hide and native fullscreen exit semantics.

import { describe, expect, it, vi } from 'vitest'
import { MainWindowLifecycleController, type LifecycleMainWindow } from './mainWindowLifecycle'

function createWindow(
  options: { destroyed?: boolean; fullScreen?: boolean; minimized?: boolean } = {}
): {
  focus: ReturnType<typeof vi.fn>
  handle: LifecycleMainWindow
  hide: ReturnType<typeof vi.fn>
  restore: ReturnType<typeof vi.fn>
  setFullScreen: ReturnType<typeof vi.fn>
  show: ReturnType<typeof vi.fn>
} {
  const focus = vi.fn()
  const hide = vi.fn()
  const restore = vi.fn()
  const setFullScreen = vi.fn()
  const show = vi.fn()
  return {
    focus,
    handle: {
      focus,
      hide,
      isDestroyed: () => options.destroyed ?? false,
      isFullScreen: () => options.fullScreen ?? false,
      isMinimized: () => options.minimized ?? false,
      restore,
      setFullScreen,
      show
    },
    hide,
    restore,
    setFullScreen,
    show
  }
}

describe('main window lifecycle', () => {
  it('hides ordinary macOS closes but allows real quits and other platforms to close', () => {
    const macController = new MainWindowLifecycleController('darwin')
    const macWindow = createWindow()

    expect(macController.requestClose(macWindow.handle, false)).toBe(true)
    expect(macWindow.hide).toHaveBeenCalledOnce()
    expect(macController.requestClose(macWindow.handle, true)).toBe(false)

    const windowsWindow = createWindow()
    expect(
      new MainWindowLifecycleController('win32').requestClose(windowsWindow.handle, false)
    ).toBe(false)
    expect(windowsWindow.hide).not.toHaveBeenCalled()
  })

  it('leaves native fullscreen before hiding the macOS window', () => {
    const controller = new MainWindowLifecycleController('darwin')
    const window = createWindow({ fullScreen: true })

    expect(controller.requestClose(window.handle, false)).toBe(true)
    expect(window.setFullScreen).toHaveBeenCalledWith(false)
    expect(window.hide).not.toHaveBeenCalled()

    controller.handleLeaveFullScreen(window.handle, false)
    expect(window.hide).toHaveBeenCalledOnce()
  })

  it('reopens a fullscreen-hidden window in normal windowed mode', () => {
    const controller = new MainWindowLifecycleController('darwin')
    const window = createWindow({ fullScreen: true })

    controller.requestClose(window.handle, false)
    controller.handleLeaveFullScreen(window.handle, false)
    controller.showExisting(window.handle)

    expect(window.show).toHaveBeenCalledOnce()
    expect(window.focus).toHaveBeenCalledOnce()
    expect(window.setFullScreen).toHaveBeenCalledOnce()
    expect(window.setFullScreen).toHaveBeenCalledWith(false)
  })

  it('cancels the pending hide when activation races the fullscreen exit', () => {
    const controller = new MainWindowLifecycleController('darwin')
    const window = createWindow({ fullScreen: true })

    controller.requestClose(window.handle, false)
    controller.showExisting(window.handle)
    controller.handleLeaveFullScreen(window.handle, false)

    expect(window.hide).not.toHaveBeenCalled()
    expect(window.show).toHaveBeenCalledOnce()
    expect(window.focus).toHaveBeenCalledOnce()
    expect(window.setFullScreen).toHaveBeenCalledOnce()
    expect(window.setFullScreen).toHaveBeenCalledWith(false)
  })

  it('restores a minimized window before showing it', () => {
    const controller = new MainWindowLifecycleController('darwin')
    const window = createWindow({ minimized: true })

    expect(controller.showExisting(window.handle)).toBe(window.handle)
    expect(window.restore).toHaveBeenCalledOnce()
    expect(window.show).toHaveBeenCalledOnce()
    expect(window.focus).toHaveBeenCalledOnce()
  })

  it('does not hide after fullscreen exit once a real quit has started', () => {
    const controller = new MainWindowLifecycleController('darwin')
    const window = createWindow({ fullScreen: true })

    controller.requestClose(window.handle, false)
    controller.prepareForQuit()
    controller.handleLeaveFullScreen(window.handle, true)

    expect(window.hide).not.toHaveBeenCalled()
  })

  it('requests a replacement for missing or destroyed windows', () => {
    const controller = new MainWindowLifecycleController('darwin')
    const destroyedWindow = createWindow({ destroyed: true })

    expect(controller.showExisting(null)).toBeNull()
    expect(controller.showExisting(destroyedWindow.handle)).toBeNull()
    expect(destroyedWindow.show).not.toHaveBeenCalled()
    expect(destroyedWindow.focus).not.toHaveBeenCalled()
  })
})
