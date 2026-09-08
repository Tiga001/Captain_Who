import type { Event, WebContents } from 'electron'

/** Keep the opener operation alive through the original native navigation, including POST. */
export function observeNativePopupLoad(guest: WebContents): {
  settled: Promise<void>
  start: () => void
  dispose: () => void
} {
  let finished = false
  let navigated = false
  let blankTimer: ReturnType<typeof setTimeout> | undefined
  let resolveSettled = (): void => undefined
  const settled = new Promise<void>((resolve) => {
    resolveSettled = resolve
  })
  const dispose = (): void => {
    if (finished) return
    finished = true
    if (blankTimer) clearTimeout(blankTimer)
    guest.removeListener('did-start-navigation', onNavigation)
    guest.removeListener('did-finish-load', onLoaded)
    guest.removeListener('did-fail-load', onFailed)
    guest.removeListener('destroyed', dispose)
    resolveSettled()
  }
  const onNavigation = (
    _event: Event,
    url: string,
    _inPlace: boolean,
    mainFrame: boolean
  ): void => {
    if (mainFrame && url !== 'about:blank' && url !== '') {
      navigated = true
      if (blankTimer) clearTimeout(blankTimer)
    }
  }
  const onLoaded = (): void => {
    if (navigated) dispose()
  }
  const onFailed = (
    _event: Event,
    code: number,
    _description: string,
    _url: string,
    mainFrame: boolean
  ): void => {
    if (mainFrame && code !== -3 && navigated) dispose()
  }
  guest.on('did-start-navigation', onNavigation)
  guest.on('did-finish-load', onLoaded)
  guest.on('did-fail-load', onFailed)
  guest.once('destroyed', dispose)
  return {
    settled,
    dispose,
    start: () => {
      if (finished || navigated || blankTimer) return
      // window.open('') followed by location assignment must retain the same native window and
      // owner. Give that assignment a bounded grace period after admission; a genuinely blank
      // popup cannot keep a Tool alive forever waiting for a navigation that may never happen.
      blankTimer = setTimeout(() => {
        if (!navigated) dispose()
      }, 1_000)
      blankTimer.unref()
    }
  }
}
