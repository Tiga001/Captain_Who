// Electron main process: keep the macOS main window alive across close-to-hide cycles.

export interface LifecycleMainWindow {
  focus(): void
  hide(): void
  isDestroyed(): boolean
  isFullScreen(): boolean
  isMinimized(): boolean
  restore(): void
  setFullScreen(fullScreen: boolean): void
  show(): void
}

/**
 * Coordinates close-to-hide with macOS native fullscreen Spaces.
 *
 * Native fullscreen transitions are asynchronous. Hiding the BrowserWindow before macOS has
 * emitted `leave-full-screen` leaves the user looking at an empty fullscreen Space. The controller
 * therefore owns the pending transition. Reopening reuses the same live Renderer in a normal
 * window instead of forcing the user back into fullscreen.
 */
export class MainWindowLifecycleController {
  private hideAfterFullScreenExit = false
  private reactivateAfterFullScreenExit = false

  constructor(private readonly platform: NodeJS.Platform) {}

  /** Returns true when the caller must prevent Electron's native close. */
  requestClose(window: LifecycleMainWindow, isQuitting: boolean): boolean {
    if (this.platform !== 'darwin' || isQuitting) return false

    if (this.hideAfterFullScreenExit) return true

    if (window.isFullScreen()) {
      this.hideAfterFullScreenExit = true
      this.reactivateAfterFullScreenExit = false
      window.setFullScreen(false)
      return true
    }

    window.hide()
    return true
  }

  handleLeaveFullScreen(window: LifecycleMainWindow, isQuitting: boolean): void {
    if (!this.hideAfterFullScreenExit) return

    this.hideAfterFullScreenExit = false
    if (isQuitting) {
      this.resetPendingState()
      return
    }

    if (this.reactivateAfterFullScreenExit) {
      this.reactivateAfterFullScreenExit = false
      this.showExisting(window)
      return
    }

    window.hide()
  }

  showExisting<T extends LifecycleMainWindow>(window: T | null): T | null {
    if (!window || window.isDestroyed()) return null

    // Activation can race the asynchronous fullscreen exit. Record the request and wait for
    // `leave-full-screen` before showing the same Renderer as a normal window. Calling show()
    // during the native Space transition can leave macOS displaying a transient black surface.
    if (this.hideAfterFullScreenExit) {
      this.reactivateAfterFullScreenExit = true
      return window
    }

    if (window.isMinimized()) window.restore()
    window.show()
    window.focus()

    return window
  }

  prepareForQuit(): void {
    this.resetPendingState()
  }

  private resetPendingState(): void {
    this.hideAfterFullScreenExit = false
    this.reactivateAfterFullScreenExit = false
  }
}
