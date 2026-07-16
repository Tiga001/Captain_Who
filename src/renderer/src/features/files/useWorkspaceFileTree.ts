import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useFileTree } from '@pierre/trees/react'
import type { WorkspaceDirectoryEntry, WorkspaceDirectoryListing } from '@mycopilot/protocol'
import { listWorkspaceDirectory } from './filesClient'

type DirectoryLoadState =
  | { status: 'loading' }
  | { status: 'ready'; value: WorkspaceDirectoryListing }
  | { status: 'error' }

interface UseWorkspaceFileTreeOptions {
  isActive: boolean
  onFileSelect: (path: string) => void
  projectId: string
  selectedPath: string | null
}

export const WORKSPACE_FILE_TREE_UNSAFE_CSS = `
  [data-file-tree-virtualized-scroll="true"] {
    overscroll-behavior: contain;
  }

  [data-type="item"] {
    letter-spacing: 0;
  }

  [data-type="item"][data-item-selected] {
    font-weight: 500;
  }
`

export function useWorkspaceFileTree({
  isActive,
  onFileSelect,
  projectId,
  selectedPath
}: UseWorkspaceFileTreeOptions) {
  const [directoryStates, setDirectoryStates] = useState<Record<string, DirectoryLoadState>>({})
  const directoryStatesRef = useRef(directoryStates)
  const directoryPathsRef = useRef<readonly string[]>([])
  const generationRef = useRef(0)
  const requestSequenceRef = useRef(0)
  const activeRequestIdsRef = useRef(new Map<string, number>())
  const lastRevealedPathRef = useRef<string | null>(null)
  const suppressSelectionRef = useRef(false)
  const selectionHandlerRef = useRef<(paths: readonly string[]) => void>(() => undefined)

  const { model } = useFileTree({
    fileTreeSearchMode: 'hide-non-matches',
    flattenEmptyDirectories: false,
    icons: { colored: true, set: 'complete' },
    initialExpansion: 'closed',
    itemHeight: 28,
    onSelectionChange: (paths) => selectionHandlerRef.current(paths),
    overscan: 24,
    paths: [],
    search: false,
    stickyFolders: false,
    unsafeCSS: WORKSPACE_FILE_TREE_UNSAFE_CSS
  })

  const loadDirectory = useCallback(
    async (directoryPath: string): Promise<void> => {
      const generation = generationRef.current
      const requestId = requestSequenceRef.current + 1
      requestSequenceRef.current = requestId
      activeRequestIdsRef.current.set(directoryPath, requestId)
      setDirectoryStates((current) => ({
        ...current,
        [directoryPath]: { status: 'loading' }
      }))

      try {
        const listing = await listWorkspaceDirectory({
          directoryPath,
          includeHidden: true,
          projectId
        })
        if (
          generationRef.current !== generation ||
          activeRequestIdsRef.current.get(directoryPath) !== requestId
        ) {
          return
        }
        setDirectoryStates((current) => ({
          ...current,
          [directoryPath]: { status: 'ready', value: listing }
        }))
      } catch {
        if (
          generationRef.current !== generation ||
          activeRequestIdsRef.current.get(directoryPath) !== requestId
        ) {
          return
        }
        setDirectoryStates((current) => ({
          ...current,
          [directoryPath]: { status: 'error' }
        }))
      }
    },
    [projectId]
  )

  useEffect(() => {
    const activeRequestIds = activeRequestIdsRef.current
    generationRef.current += 1
    activeRequestIds.clear()
    setDirectoryStates({})
    lastRevealedPathRef.current = null
    model.resetPaths([])
    return () => {
      generationRef.current += 1
      activeRequestIds.clear()
    }
  }, [model, projectId])

  useEffect(() => {
    directoryStatesRef.current = directoryStates
  }, [directoryStates])

  const rootState = directoryStates['']

  useEffect(() => {
    if (isActive && !rootState && !activeRequestIdsRef.current.has('')) void loadDirectory('')
  }, [isActive, loadDirectory, rootState])

  const entryByPath = useMemo(() => {
    const entries = new Map<string, WorkspaceDirectoryEntry>()
    for (const state of Object.values(directoryStates)) {
      if (state.status !== 'ready') continue
      for (const entry of state.value.entries) entries.set(entry.path, entry)
    }
    return entries
  }, [directoryStates])

  const paths = useMemo(() => [...entryByPath.keys()], [entryByPath])
  const directoryPaths = useMemo(
    () =>
      [...entryByPath.values()]
        .filter((entry) => entry.kind === 'directory')
        .map((entry) => entry.path),
    [entryByPath]
  )

  useEffect(() => {
    directoryPathsRef.current = directoryPaths
  }, [directoryPaths])

  const syncSelection = useCallback(() => {
    const selectedPaths = model.getSelectedPaths()
    if (
      selectedPaths.length === (selectedPath ? 1 : 0) &&
      (!selectedPath || selectedPaths[0] === selectedPath)
    ) {
      return
    }

    suppressSelectionRef.current = true
    for (const path of selectedPaths) model.getItem(path)?.deselect()
    if (selectedPath) model.getItem(selectedPath)?.select()
    queueMicrotask(() => {
      suppressSelectionRef.current = false
    })
  }, [model, selectedPath])

  useEffect(() => {
    selectionHandlerRef.current = (selectedPaths) => {
      if (suppressSelectionRef.current) return
      const path = selectedPaths.at(-1)
      if (!path) return
      const entry = entryByPath.get(path)
      if (!entry || entry.kind === 'directory') return
      onFileSelect(path)
      queueMicrotask(syncSelection)
    }
  }, [entryByPath, onFileSelect, syncSelection])

  useEffect(() => {
    const expandedPaths = directoryPaths.filter((path) => {
      const item = model.getItem(path)
      return item !== null && 'isExpanded' in item && item.isExpanded()
    })
    model.resetPaths(paths, { initialExpandedPaths: expandedPaths })
  }, [directoryPaths, model, paths])

  useEffect(() => {
    const loadExpandedDirectories = (): void => {
      if (!isActive) return
      for (const path of directoryPathsRef.current) {
        const item = model.getItem(path)
        if (!item || !('isExpanded' in item) || !item.isExpanded()) continue
        const directoryPath = path.slice(0, -1)
        if (
          !directoryStatesRef.current[directoryPath] &&
          !activeRequestIdsRef.current.has(directoryPath)
        ) {
          void loadDirectory(directoryPath)
        }
      }
    }

    const unsubscribe = model.subscribe(loadExpandedDirectories)
    loadExpandedDirectories()
    return unsubscribe
  }, [isActive, loadDirectory, model])

  useEffect(() => {
    if (!isActive) return

    if (!selectedPath) {
      syncSelection()
      return
    }

    let directoryPath = ''
    for (const segment of selectedPath.split('/').slice(0, -1)) {
      directoryPath = `${directoryPath}${segment}/`
      const directory = model.getItem(directoryPath)
      if (!directory || !('isExpanded' in directory)) return
      if (!directory.isExpanded()) {
        directory.expand()
        return
      }
    }

    if (!model.getItem(selectedPath)) return
    syncSelection()
    if (lastRevealedPathRef.current !== selectedPath) {
      lastRevealedPathRef.current = selectedPath
      model.scrollToPath(selectedPath, { focus: false, offset: 'nearest' })
    }
  }, [directoryStates, isActive, model, selectedPath, syncSelection])

  const refresh = useCallback(() => {
    generationRef.current += 1
    activeRequestIdsRef.current.clear()
    setDirectoryStates({})
    model.resetPaths([])
    if (isActive) void loadDirectory('')
  }, [isActive, loadDirectory, model])

  const failedDirectoryCount = Object.values(directoryStates).filter(
    (state) => state.status === 'error'
  ).length
  const hasTruncatedDirectory = Object.values(directoryStates).some(
    (state) => state.status === 'ready' && state.value.truncated
  )

  return {
    failedDirectoryCount,
    hasTruncatedDirectory,
    model,
    refresh,
    rootState
  }
}
