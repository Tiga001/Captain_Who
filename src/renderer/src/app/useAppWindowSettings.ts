import { useCallback, useEffect, useRef, useState, type RefObject } from 'react'
import type { AppWindowState } from '@mycopilot/host-api'
import { hostClient } from '../host/hostClient'
import type { SettingsPageId } from '../features/settings/SettingsPage'
import type { BrowserAutomationView } from '../features/mcp/BrowserAutomationSettingsPage'
import { DEFAULT_APP_WINDOW_STATE } from './appShellConversationUtils'

export function useAppWindowSettings<T extends HTMLElement>(shellRef: RefObject<T | null>) {
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [settingsInitialPage, setSettingsInitialPage] = useState<SettingsPageId>('general')
  const [settingsInitialBrowserView, setSettingsInitialBrowserView] =
    useState<BrowserAutomationView>()
  const [appWindowState, setAppWindowState] = useState<AppWindowState>(DEFAULT_APP_WINDOW_STATE)
  const workspaceFocusBeforeSettingsRef = useRef<HTMLElement | null>(null)

  useEffect(() => {
    let cancelled = false
    const unsubscribe = hostClient.app.onWindowStateChange(setAppWindowState)

    void hostClient.app
      .getWindowState()
      .then((state) => {
        if (!cancelled) setAppWindowState(state)
      })
      .catch((error) => {
        console.error('Failed to load app window state', error)
      })

    return () => {
      cancelled = true
      unsubscribe()
    }
  }, [])

  const openSettings = useCallback(
    (initialPage: SettingsPageId = 'general', browserView?: BrowserAutomationView) => {
      const activeElement = document.activeElement
      workspaceFocusBeforeSettingsRef.current =
        activeElement instanceof HTMLElement && shellRef.current?.contains(activeElement)
          ? activeElement
          : null
      setSettingsInitialPage(initialPage)
      setSettingsInitialBrowserView(initialPage === 'browser' ? browserView : undefined)
      setSettingsOpen(true)
    },
    [shellRef]
  )

  const closeSettings = useCallback(() => {
    const previousWorkspaceFocus = workspaceFocusBeforeSettingsRef.current
    workspaceFocusBeforeSettingsRef.current = null
    setSettingsOpen(false)

    window.requestAnimationFrame(() => {
      if (previousWorkspaceFocus?.isConnected) {
        previousWorkspaceFocus.focus({ preventScroll: true })
      }
    })
  }, [])

  return {
    appWindowMaximized: appWindowState.isFullScreen || appWindowState.isMaximized,
    closeSettings,
    openSettings,
    setSettingsOpen,
    settingsInitialPage,
    settingsInitialBrowserView,
    settingsOpen
  }
}
