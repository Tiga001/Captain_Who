import { useSyncExternalStore } from 'react'
import type { RightSidebarActivity } from './rightSidebarTypes'

export interface RightSidebarActivityInput {
  documentVisible: boolean
  isSelected: boolean
  sidebarVisible: boolean
}

export function resolveRightSidebarActivity({
  documentVisible,
  isSelected,
  sidebarVisible
}: RightSidebarActivityInput): RightSidebarActivity {
  if (!sidebarVisible || !documentVisible) return 'dormant'
  return isSelected ? 'foreground' : 'background'
}

export function useRightSidebarDocumentVisibility(): boolean {
  return useSyncExternalStore(
    subscribeToDocumentVisibility,
    getDocumentVisibilitySnapshot,
    getServerDocumentVisibilitySnapshot
  )
}

function subscribeToDocumentVisibility(onStoreChange: () => void): () => void {
  document.addEventListener('visibilitychange', onStoreChange)
  return () => document.removeEventListener('visibilitychange', onStoreChange)
}

function getDocumentVisibilitySnapshot(): boolean {
  return document.visibilityState === 'visible'
}

function getServerDocumentVisibilitySnapshot(): boolean {
  return true
}
