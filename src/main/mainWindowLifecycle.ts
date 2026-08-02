// Electron main process: keep the macOS main window alive when the user closes it.

export interface RestorableMainWindow {
  focus(): void
  isDestroyed(): boolean
  isMinimized(): boolean
  restore(): void
  show(): void
}

export function shouldHideMainWindowOnClose(
  platform: NodeJS.Platform,
  isQuitting: boolean
): boolean {
  return platform === 'darwin' && !isQuitting
}

export function showExistingMainWindow<T extends RestorableMainWindow>(window: T | null): T | null {
  if (!window || window.isDestroyed()) return null

  if (window.isMinimized()) window.restore()
  window.show()
  window.focus()
  return window
}
