import { createContext, useContext, useEffect, useRef, type ReactNode } from 'react'

export interface SettingsNavigationTarget {
  page: string
  id: string
  view?: string
  prerequisiteId?: string
  ancestorIds?: readonly string[]
  revision: number
}

const SettingsSearchTargetContext = createContext<SettingsNavigationTarget | null>(null)

export function SettingsSearchNavigationProvider({
  target,
  children
}: {
  target: SettingsNavigationTarget | null
  children: ReactNode
}) {
  return (
    <SettingsSearchTargetContext.Provider value={target}>
      {children}
    </SettingsSearchTargetContext.Provider>
  )
}

/** Runs on an explicit result selection, including repeated selections within a page. */
export function useSettingsPageNavigation(
  page: string,
  navigate: (target: SettingsNavigationTarget) => void,
  cancel?: () => void
): void {
  const target = useContext(SettingsSearchTargetContext)
  const navigateRef = useRef(navigate)
  const cancelRef = useRef(cancel)
  useEffect(() => {
    navigateRef.current = navigate
    cancelRef.current = cancel
  })
  useEffect(() => {
    if (target?.page === page) navigateRef.current(target)
    else cancelRef.current?.()
  }, [page, target])
}
