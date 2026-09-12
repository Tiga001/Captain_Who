import { FileTree } from '@pierre/trees'
import type { WorkspaceDirectoryEntry, WorkspaceDirectoryListing } from '@mycopilot/protocol'
import { listWorkspaceDirectory } from './filesClient'

export type WorkspaceDirectoryLoadState =
  | { status: 'loading' }
  | { status: 'ready'; value: WorkspaceDirectoryListing }
  | { status: 'error' }

export interface WorkspaceFileTreeSnapshot {
  failedDirectoryCount: number
  hasTruncatedDirectory: boolean
  isTreeVisible: boolean
  rootState: WorkspaceDirectoryLoadState | undefined
  searchQuery: string
}

export interface WorkspaceFileTreeConsumer {
  isActive: boolean
  onFileSelect: (path: string) => void
  selectedPath: string | null
}

export const WORKSPACE_FILE_TREE_UNSAFE_CSS = `
  [data-file-tree-virtualized-scroll="true"] {
    overscroll-behavior: contain;
  }

  [data-file-tree-sticky-overlay-content="true"] {
    background-color: var(--trees-bg);
    box-shadow:
      0 1px 0 var(--trees-border-color),
      0 8px 14px -14px color-mix(in srgb, var(--trees-fg) 38%, transparent);
  }

  [data-type="item"][data-file-tree-sticky-row="true"] {
    background-color: var(--trees-bg);
  }

  [data-type="item"][data-file-tree-sticky-row="true"][data-item-focused="true"]::before {
    outline: 0;
  }

  [data-type="item"] {
    letter-spacing: 0;
  }

  [data-type="item"][data-item-selected] {
    font-weight: 500;
  }
`

export class WorkspaceFileTreeSession {
  readonly model: FileTree

  private activeConsumerToken: symbol | null = null
  private readonly activeRequestIds = new Map<string, number>()
  private readonly consumers = new Map<symbol, WorkspaceFileTreeConsumer>()
  private destroyed = false
  private directoryPaths: readonly string[] = []
  private directoryStates: Record<string, WorkspaceDirectoryLoadState> = {}
  private entryByPath = new Map<string, WorkspaceDirectoryEntry>()
  private generation = 0
  private lastRevealedPath: string | null = null
  private readonly listeners = new Set<() => void>()
  private requestSequence = 0
  private snapshot: WorkspaceFileTreeSnapshot
  private suppressSelection = false
  private readonly unsubscribeModel: () => void

  constructor(
    readonly projectId: string,
    readonly folderId?: string
  ) {
    this.model = new FileTree({
      fileTreeSearchMode: 'hide-non-matches',
      flattenEmptyDirectories: false,
      icons: { colored: true, set: 'complete' },
      initialExpansion: 'closed',
      itemHeight: 28,
      onSelectionChange: (paths) => this.handleSelectionChange(paths),
      overscan: 24,
      paths: [],
      search: false,
      stickyFolders: true,
      unsafeCSS: WORKSPACE_FILE_TREE_UNSAFE_CSS
    })
    this.snapshot = this.createSnapshot(true, '')
    this.unsubscribeModel = this.model.subscribe(this.handleModelChange)
  }

  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  readonly getSnapshot = (): WorkspaceFileTreeSnapshot => this.snapshot

  attachConsumer(token: symbol, consumer: WorkspaceFileTreeConsumer): void {
    if (this.destroyed) return
    this.consumers.set(token, consumer)
    this.selectActiveConsumer(token, consumer)
    this.synchronizeActiveConsumer()
  }

  updateConsumer(token: symbol, consumer: WorkspaceFileTreeConsumer): void {
    if (this.destroyed || !this.consumers.has(token)) return
    this.consumers.set(token, consumer)
    this.selectActiveConsumer(token, consumer)
    this.synchronizeActiveConsumer()
  }

  detachConsumer(token: symbol): void {
    this.consumers.delete(token)
    if (this.activeConsumerToken !== token) return

    this.activeConsumerToken = null
    for (const [candidateToken, consumer] of this.consumers) {
      if (consumer.isActive) this.activeConsumerToken = candidateToken
    }
    this.synchronizeActiveConsumer()
  }

  readonly setSearchQuery = (searchQuery: string): void => {
    if (this.destroyed || searchQuery === this.snapshot.searchQuery) return
    this.model.setSearch(searchQuery.trim() || null)
    this.publishSnapshot({ searchQuery })
  }

  readonly setTreeVisible = (isTreeVisible: boolean): void => {
    if (this.destroyed || isTreeVisible === this.snapshot.isTreeVisible) return
    this.publishSnapshot({ isTreeVisible })
  }

  readonly refresh = (): void => {
    if (this.destroyed) return
    this.generation += 1
    this.activeRequestIds.clear()
    this.directoryStates = {}
    this.directoryPaths = []
    this.entryByPath = new Map()
    this.lastRevealedPath = null
    this.suppressModelSelection()
    this.model.resetPaths([])
    this.publishSnapshot()
    this.synchronizeActiveConsumer()
  }

  destroy(): void {
    if (this.destroyed) return
    this.destroyed = true
    this.generation += 1
    this.activeRequestIds.clear()
    this.consumers.clear()
    this.listeners.clear()
    this.unsubscribeModel()
    this.model.cleanUp()
  }

  private selectActiveConsumer(token: symbol, consumer: WorkspaceFileTreeConsumer): void {
    if (consumer.isActive) {
      this.activeConsumerToken = token
    } else if (this.activeConsumerToken === token) {
      this.activeConsumerToken = null
    }
  }

  private getActiveConsumer(): WorkspaceFileTreeConsumer | null {
    if (!this.activeConsumerToken) return null
    const consumer = this.consumers.get(this.activeConsumerToken)
    return consumer?.isActive ? consumer : null
  }

  private synchronizeActiveConsumer(): void {
    const consumer = this.getActiveConsumer()
    if (!consumer) return

    if (!this.directoryStates[''] && !this.activeRequestIds.has('')) {
      void this.loadDirectory('')
    }
    this.revealSelectedPath(consumer.selectedPath)
  }

  private readonly handleModelChange = (): void => {
    const consumer = this.getActiveConsumer()
    if (!consumer) return

    for (const path of this.directoryPaths) {
      const item = this.model.getItem(path)
      if (!item || !('isExpanded' in item) || !item.isExpanded()) continue

      const directoryPath = path.slice(0, -1)
      if (!this.directoryStates[directoryPath] && !this.activeRequestIds.has(directoryPath)) {
        void this.loadDirectory(directoryPath)
      }
    }
  }

  private handleSelectionChange(selectedPaths: readonly string[]): void {
    if (this.destroyed || this.suppressSelection) return

    const path = selectedPaths.at(-1)
    const consumer = this.getActiveConsumer()
    if (!path || !consumer) return

    const entry = this.entryByPath.get(path)
    if (!entry || entry.kind === 'directory') return

    consumer.onFileSelect(path)
    queueMicrotask(() => {
      const activeConsumer = this.getActiveConsumer()
      if (!this.destroyed && activeConsumer) {
        this.synchronizeSelection(activeConsumer.selectedPath)
      }
    })
  }

  private async loadDirectory(directoryPath: string): Promise<void> {
    const generation = this.generation
    const requestId = this.requestSequence + 1
    this.requestSequence = requestId
    this.activeRequestIds.set(directoryPath, requestId)
    this.directoryStates = {
      ...this.directoryStates,
      [directoryPath]: { status: 'loading' }
    }
    this.publishSnapshot()

    try {
      const listing = await listWorkspaceDirectory({
        directoryPath,
        ...(this.folderId === undefined ? {} : { folderId: this.folderId }),
        includeHidden: true,
        projectId: this.projectId
      })
      if (!this.isCurrentRequest(directoryPath, generation, requestId)) return

      this.activeRequestIds.delete(directoryPath)
      this.directoryStates = {
        ...this.directoryStates,
        [directoryPath]: { status: 'ready', value: listing }
      }
      this.rebuildModel()
      this.publishSnapshot()
      this.synchronizeActiveConsumer()
    } catch {
      if (!this.isCurrentRequest(directoryPath, generation, requestId)) return

      this.activeRequestIds.delete(directoryPath)
      this.directoryStates = {
        ...this.directoryStates,
        [directoryPath]: { status: 'error' }
      }
      this.publishSnapshot()
    }
  }

  private isCurrentRequest(directoryPath: string, generation: number, requestId: number): boolean {
    return (
      !this.destroyed &&
      this.generation === generation &&
      this.activeRequestIds.get(directoryPath) === requestId
    )
  }

  private rebuildModel(): void {
    const expandedPaths = this.directoryPaths.filter((path) => {
      const item = this.model.getItem(path)
      return item !== null && 'isExpanded' in item && item.isExpanded()
    })
    const entries = new Map<string, WorkspaceDirectoryEntry>()
    for (const state of Object.values(this.directoryStates)) {
      if (state.status !== 'ready') continue
      for (const entry of state.value.entries) entries.set(entry.path, entry)
    }

    this.entryByPath = entries
    this.directoryPaths = [...entries.values()]
      .filter((entry) => entry.kind === 'directory')
      .map((entry) => entry.path)
    this.suppressModelSelection()
    this.model.resetPaths([...entries.keys()], { initialExpandedPaths: expandedPaths })
  }

  private revealSelectedPath(selectedPath: string | null): void {
    if (!selectedPath) {
      this.synchronizeSelection(null)
      return
    }

    let directoryPath = ''
    for (const segment of selectedPath.split('/').slice(0, -1)) {
      directoryPath = `${directoryPath}${segment}/`
      const directory = this.model.getItem(directoryPath)
      if (!directory || !('isExpanded' in directory)) return
      if (!directory.isExpanded()) directory.expand()
    }

    if (!this.model.getItem(selectedPath)) return
    this.synchronizeSelection(selectedPath)
    if (this.lastRevealedPath !== selectedPath && this.model.getFileTreeContainer() !== undefined) {
      this.lastRevealedPath = selectedPath
      this.model.scrollToPath(selectedPath, { focus: false, offset: 'nearest' })
    }
  }

  private synchronizeSelection(selectedPath: string | null): void {
    const selectedPaths = this.model.getSelectedPaths()
    if (
      selectedPaths.length === (selectedPath ? 1 : 0) &&
      (!selectedPath || selectedPaths[0] === selectedPath)
    ) {
      return
    }

    this.suppressModelSelection()
    for (const path of selectedPaths) this.model.getItem(path)?.deselect()
    if (selectedPath) this.model.getItem(selectedPath)?.select()
  }

  private suppressModelSelection(): void {
    this.suppressSelection = true
    queueMicrotask(() => {
      this.suppressSelection = false
    })
  }

  private createSnapshot(
    isTreeVisible = this.snapshot?.isTreeVisible ?? true,
    searchQuery = this.snapshot?.searchQuery ?? ''
  ): WorkspaceFileTreeSnapshot {
    const states = Object.values(this.directoryStates)
    return {
      failedDirectoryCount: states.filter((state) => state.status === 'error').length,
      hasTruncatedDirectory: states.some(
        (state) => state.status === 'ready' && state.value.truncated
      ),
      isTreeVisible,
      rootState: this.directoryStates[''],
      searchQuery
    }
  }

  private publishSnapshot(
    update: Partial<Pick<WorkspaceFileTreeSnapshot, 'isTreeVisible' | 'searchQuery'>> = {}
  ): void {
    this.snapshot = this.createSnapshot(
      update.isTreeVisible ?? this.snapshot.isTreeVisible,
      update.searchQuery ?? this.snapshot.searchQuery
    )
    for (const listener of this.listeners) listener()
  }
}
