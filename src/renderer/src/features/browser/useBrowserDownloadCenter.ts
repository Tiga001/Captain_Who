import { unwrapHostInvocation, type BrowserHostApi } from '@mycopilot/host-api'
import {
  BROWSER_DOWNLOAD_SCHEMA_VERSION,
  type BrowserDownloadCenterAction,
  type BrowserDownloadCenterSnapshot
} from '@mycopilot/protocol'
import { useCallback, useEffect, useRef, useState } from 'react'

import { resolveBrowserSurfaceHostApi } from './browserSurface'

const EMPTY_SNAPSHOT: BrowserDownloadCenterSnapshot = {
  schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
  revision: 0,
  downloads: []
}

export function useBrowserDownloadCenter() {
  const [snapshot, setSnapshot] = useState<BrowserDownloadCenterSnapshot>(EMPTY_SNAPSHOT)
  const revisionRef = useRef(0)

  const applySnapshot = useCallback((next: BrowserDownloadCenterSnapshot): void => {
    if (next.revision < revisionRef.current) return
    revisionRef.current = next.revision
    setSnapshot(next)
  }, [])

  useEffect(() => {
    const browser = resolveDownloadCenterHostApi()
    if (!browser) return
    let disposed = false
    const unsubscribe = browser.onDownloadCenterChanged((next) => {
      if (!disposed) applySnapshot(next)
    })
    void browser
      .getDownloadCenter()
      .then(unwrapHostInvocation)
      .then((next) => {
        if (!disposed) applySnapshot(next)
      })
      .catch(() => undefined)
    return () => {
      disposed = true
      unsubscribe()
    }
  }, [applySnapshot])

  const perform = useCallback(
    async (
      downloadId: string,
      action: BrowserDownloadCenterAction
    ): Promise<'performed' | 'unavailable'> => {
      const browser = resolveDownloadCenterHostApi()
      if (!browser) return 'unavailable'
      try {
        const output = unwrapHostInvocation(
          await browser.performDownloadCenterAction({
            schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
            downloadId,
            action
          })
        )
        applySnapshot(output.snapshot)
        return output.status
      } catch {
        return 'unavailable'
      }
    },
    [applySnapshot]
  )

  const openDirectory = useCallback(async (): Promise<'opened' | 'unavailable'> => {
    const browser = resolveDownloadCenterHostApi()
    if (!browser) return 'unavailable'
    try {
      return unwrapHostInvocation(await browser.openDownloadDirectory()).status
    } catch {
      return 'unavailable'
    }
  }, [])

  return { openDirectory, perform, snapshot }
}

function resolveDownloadCenterHostApi(): BrowserHostApi | null {
  const browser = resolveBrowserSurfaceHostApi()
  if (
    !browser ||
    typeof browser.getDownloadCenter !== 'function' ||
    typeof browser.onDownloadCenterChanged !== 'function' ||
    typeof browser.openDownloadDirectory !== 'function' ||
    typeof browser.performDownloadCenterAction !== 'function'
  ) {
    return null
  }
  return browser
}
