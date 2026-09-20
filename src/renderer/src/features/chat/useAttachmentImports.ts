import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import type { AgentInputAttachment, AttachmentImportProgress } from '@mycopilot/protocol'
import { hostClient } from '../../host/hostClient'
import {
  composerAttachmentFromAgentAttachment,
  createComposerAttachmentsFromFiles,
  loadComposerAttachmentPreview,
  selectComposerAttachments,
  type ComposerAttachment,
  type ComposerAttachmentKind
} from './chatAttachments'

export interface PendingAttachmentImport {
  id: string
  kind: ComposerAttachmentKind
  name: string
  sizeBytes: number
  importState: 'importing' | 'failed'
  importedBytes: number
  error?: string
}

interface ImportEntry {
  value: PendingAttachmentImport
  file?: File
  fileKey?: string
  controller: AbortController
  scope: string
  requestId?: string
}

interface AttachmentImportsOptions {
  scope: string
  /** Attachments already present in the current composer draft. */
  existingAttachments: readonly ComposerAttachment[]
  onAttachments: (attachments: ComposerAttachment[]) => void
  onError: (error: unknown) => void
  errorMessage: (error: unknown) => string
}

function visibleProgress(value: PendingAttachmentImport): number {
  return value.sizeBytes > 0 ? Math.floor((value.importedBytes / value.sizeBytes) * 100) : 0
}

export function useAttachmentImports(options: AttachmentImportsOptions) {
  const optionsRef = useRef(options)
  const scopeVersion = useRef({ key: options.scope, version: 0 })
  useLayoutEffect(() => {
    if (scopeVersion.current.key !== options.scope) {
      scopeVersion.current = { key: options.scope, version: scopeVersion.current.version + 1 }
    }
    optionsRef.current = {
      ...options,
      scope: JSON.stringify([options.scope, scopeVersion.current.version])
    }
  })
  const entries = useRef(new Map<string, ImportEntry>())
  const cancelledIds = useRef(new Set<string>())
  const nativeRequests = useRef(new Map<string, string>())
  // File metadata is only used while a browser import is in flight. The durable content hash
  // remains the source of truth once the importer finishes; this transient key prevents a
  // multi-file drop containing the same local file from rendering duplicate pending cards.
  const pendingFileKeys = useRef(new Set<string>())
  // Managed imports receive their content identity only when the streaming importer finishes.
  // Keep the identities admitted by this hook until the parent reflects them in
  // existingAttachments; this closes the small async window between two drops without using
  // names or sizes as a false identity.
  const admittedHashes = useRef(new Map<string, string>())
  const mounted = useRef(true)
  const [pending, setPending] = useState<PendingAttachmentImport[]>([])
  const [selecting, setSelecting] = useState(false)

  useLayoutEffect(() => {
    const activeIds = new Set(options.existingAttachments.map((item) => item.id))
    for (const [hash, id] of admittedHashes.current) {
      if (!activeIds.has(id)) admittedHashes.current.delete(hash)
    }
  }, [options.existingAttachments])

  const publish = useCallback(() => {
    if (!mounted.current) return
    setPending(
      [...entries.current.values()]
        .filter((entry) => entry.scope === optionsRef.current.scope)
        .map((entry) => entry.value)
    )
    setSelecting([...nativeRequests.current.values()].includes(optionsRef.current.scope))
  }, [])

  const isCurrent = (entry: ImportEntry) =>
    mounted.current &&
    !entry.controller.signal.aborted &&
    entry.scope === optionsRef.current.scope &&
    entries.current.get(entry.value.id) === entry

  const cancel = useCallback(
    (id: string) => {
      const entry = entries.current.get(id)
      if (!entry) return
      cancelledIds.current.add(id)
      entry.controller.abort()
      entries.current.delete(id)
      if (entry.fileKey) pendingFileKeys.current.delete(entry.fileKey)
      if (!entry.file) void hostClient.attachments?.cancelImport({ importId: id }).catch(() => {})
      publish()
    },
    [publish]
  )

  const cancelAll = useCallback(() => {
    const scope = optionsRef.current.scope
    for (const [requestId, requestScope] of nativeRequests.current) {
      if (requestScope === scope) {
        void hostClient.attachments?.cancelImport({ importId: requestId }).catch(() => {})
      }
    }
    for (const [id, entry] of entries.current) {
      if (entry.scope === scope) cancel(id)
    }
    // Replacing a draft inside the same conversation (for example, editing a queued message)
    // must also invalidate a picker that has not returned any attachment IDs yet.
    scopeVersion.current.version += 1
    optionsRef.current = {
      ...optionsRef.current,
      scope: JSON.stringify([scopeVersion.current.key, scopeVersion.current.version])
    }
    publish()
  }, [cancel, publish])

  useLayoutEffect(() => {
    for (const [requestId, scope] of nativeRequests.current) {
      if (scope !== optionsRef.current.scope) {
        void hostClient.attachments?.cancelImport({ importId: requestId }).catch(() => {})
      }
    }
    for (const [id, entry] of entries.current) {
      if (entry.scope !== optionsRef.current.scope) cancel(id)
    }
    publish()
  }, [options.scope, cancel, publish])

  useEffect(() => {
    mounted.current = true
    const currentEntries = entries.current
    const currentRequests = nativeRequests.current
    const currentPendingFileKeys = pendingFileKeys.current
    return () => {
      mounted.current = false
      for (const entry of currentEntries.values()) {
        entry.controller.abort()
        if (!entry.file)
          void hostClient.attachments?.cancelImport({ importId: entry.value.id }).catch(() => {})
      }
      currentEntries.clear()
      for (const requestId of currentRequests.keys()) {
        void hostClient.attachments?.cancelImport({ importId: requestId }).catch(() => {})
      }
      currentRequests.clear()
      currentPendingFileKeys.clear()
    }
  }, [])

  const accept = (entry: ImportEntry, attachment: ComposerAttachment) => {
    if (!isCurrent(entry)) return
    entries.current.delete(entry.value.id)
    if (entry.fileKey) pendingFileKeys.current.delete(entry.fileKey)
    if (isDuplicate(attachment)) {
      publish()
      return
    }
    rememberIdentity(attachment)
    optionsRef.current.onAttachments([attachment])
    publish()
  }

  const attachmentHash = (attachment: ComposerAttachment): string | undefined => {
    const value = attachment.agentAttachment.contentSha256?.trim().toLowerCase()
    if (!value) return undefined
    if (/^[0-9a-f]{64}$/.test(value)) return `sha256:${value}`
    return /^sha256:[0-9a-f]{64}$/.test(value) ? value : undefined
  }

  const isDuplicate = (attachment: ComposerAttachment): boolean => {
    const hash = attachmentHash(attachment)
    if (!hash) return false
    return (
      optionsRef.current.existingAttachments.some((item) => attachmentHash(item) === hash) ||
      admittedHashes.current.has(hash)
    )
  }

  const rememberIdentity = (attachment: ComposerAttachment) => {
    const hash = attachmentHash(attachment)
    if (hash) admittedHashes.current.set(hash, attachment.id)
  }

  const fail = (entry: ImportEntry, error: unknown) => {
    if (!isCurrent(entry)) return
    if (entry.fileKey) pendingFileKeys.current.delete(entry.fileKey)
    entry.value = {
      ...entry.value,
      importState: 'failed',
      error: optionsRef.current.errorMessage(error)
    }
    publish()
  }

  const runFileImport = async (entry: ImportEntry) => {
    if (!entry.file || !isCurrent(entry)) return
    try {
      const attachments = await createComposerAttachmentsFromFiles([entry.file], {
        id: entry.value.id,
        signal: entry.controller.signal,
        onProgress: (progress) => {
          if (!isCurrent(entry)) return
          const previous = visibleProgress(entry.value)
          const previousKind = entry.value.kind
          entry.value = {
            ...entry.value,
            ...progress.attachment,
            importedBytes: progress.receivedBytes
          }
          // A large import can emit thousands of chunks. Its one-line card only needs updates
          // when the visible percentage changes, avoiding a full composer render per chunk.
          if (previous !== visibleProgress(entry.value) || previousKind !== entry.value.kind)
            publish()
        }
      })
      if (attachments[0]) accept(entry, attachments[0])
    } catch (error) {
      fail(entry, error)
    }
  }

  const addFiles = async (files: FileList | File[]) => {
    const newEntries: ImportEntry[] = []
    for (const file of Array.from(files)) {
      const fileKey = [file.name, file.size, file.lastModified, file.type].join('\u0000')
      if (pendingFileKeys.current.has(fileKey)) continue
      pendingFileKeys.current.add(fileKey)
      newEntries.push({
        value: {
          id: `attachment-${crypto.randomUUID()}`,
          kind:
            file.type.startsWith('image/') ||
            /\.(apng|avif|bmp|gif|heic|heif|ico|jpe?g|png|svg|tiff?|webp)$/i.test(file.name)
              ? 'image'
              : 'file',
          name: file.name || 'attachment',
          sizeBytes: file.size,
          importState: 'importing',
          importedBytes: 0
        },
        file,
        fileKey,
        controller: new AbortController(),
        scope: optionsRef.current.scope
      })
    }
    for (const entry of newEntries) entries.current.set(entry.value.id, entry)
    publish()
    for (const entry of newEntries) await runFileImport(entry)
  }

  useEffect(
    () =>
      hostClient.attachments?.onImportProgress?.((progress: AttachmentImportProgress) => {
        const existing = entries.current.get(progress.attachment.id)
        // Native picker progress is correlated with its originating draft, even if its dialog
        // finishes after the user switches conversations. Browser imports have their own callback.
        const requestId = progress.requestId
        const scope = existing?.requestId
          ? existing.scope
          : requestId
            ? nativeRequests.current.get(requestId)
            : undefined
        if (!scope || cancelledIds.current.has(progress.attachment.id)) return
        if (scope !== optionsRef.current.scope) {
          cancelledIds.current.add(progress.attachment.id)
          void hostClient.attachments
            .cancelImport({ importId: progress.attachment.id })
            .catch(() => {})
          return
        }
        const entry = existing ?? {
          value: { ...progress.attachment, importState: 'importing' as const, importedBytes: 0 },
          controller: new AbortController(),
          scope,
          requestId
        }
        if (entry.file) return
        const previous = visibleProgress(entry.value)
        const metadataChanged =
          entry.value.sizeBytes !== progress.attachment.sizeBytes ||
          entry.value.kind !== progress.attachment.kind ||
          entry.value.name !== progress.attachment.name
        if (progress.status === 'cancelled') {
          entries.current.delete(entry.value.id)
        } else {
          entry.value = {
            ...entry.value,
            ...progress.attachment,
            importedBytes: progress.receivedBytes,
            importState: progress.status === 'failed' ? 'failed' : 'importing',
            error:
              progress.status === 'failed'
                ? optionsRef.current.errorMessage(progress.error)
                : undefined
          }
          entries.current.set(entry.value.id, entry)
        }
        if (
          !existing ||
          metadataChanged ||
          progress.status !== 'importing' ||
          previous !== visibleProgress(entry.value)
        )
          publish()
      }),
    [publish]
  )

  const select = async (kind: ComposerAttachmentKind) => {
    const requestId = `attachment-selection-${crypto.randomUUID()}`
    const scope = optionsRef.current.scope
    nativeRequests.current.set(requestId, scope)
    publish()
    try {
      const result = await selectComposerAttachments(kind, requestId)
      if (!mounted.current || scope !== optionsRef.current.scope) return
      const accepted: ComposerAttachment[] = []
      for (const attachment of result) {
        entries.current.delete(attachment.id)
        if (cancelledIds.current.has(attachment.id) || isDuplicate(attachment)) continue
        rememberIdentity(attachment)
        accepted.push(attachment)
      }
      if (accepted.length) optionsRef.current.onAttachments(accepted)
    } catch (error) {
      if (mounted.current && scope === optionsRef.current.scope) {
        for (const entry of entries.current.values()) {
          if (entry.requestId === requestId && entry.value.importState === 'importing')
            fail(entry, error)
        }
        optionsRef.current.onError(error)
      }
    } finally {
      nativeRequests.current.delete(requestId)
      publish()
    }
  }

  const retry = async (id: string) => {
    const entry = entries.current.get(id)
    if (!entry || entry.value.importState !== 'failed' || !isCurrent(entry)) return
    entry.value = { ...entry.value, importState: 'importing', importedBytes: 0, error: undefined }
    publish()
    if (entry.file) {
      if (entry.fileKey) pendingFileKeys.current.add(entry.fileKey)
      await runFileImport(entry)
    } else {
      try {
        const attachment = await hostClient.attachments.retryInputAttachment({ attachmentId: id })
        accept(entry, composerAttachmentFromAgentAttachment(attachment))
      } catch (error) {
        fail(entry, error)
      }
    }
  }

  return {
    pending,
    addFiles,
    select,
    cancel,
    cancelAll,
    retry,
    hasPending: selecting || pending.length > 0
  }
}

export function useComposerAttachmentPreviews<
  T extends {
    id: string
    kind: ComposerAttachmentKind
    previewUrl?: string
    agentAttachment?: AgentInputAttachment
  }
>(attachments: T[]): T[] {
  const [previews, setPreviews] = useState<Map<string, string>>(new Map())
  const previewCache = useRef(previews)
  useEffect(() => {
    let active = true
    const ids = new Set(attachments.map((attachment) => attachment.id))
    if ([...previewCache.current.keys()].some((id) => !ids.has(id))) {
      previewCache.current = new Map([...previewCache.current].filter(([id]) => ids.has(id)))
      setPreviews(previewCache.current)
    }
    for (const attachment of attachments) {
      if (
        attachment.kind !== 'image' ||
        !attachment.agentAttachment ||
        attachment.previewUrl ||
        previewCache.current.has(attachment.id)
      )
        continue
      void loadComposerAttachmentPreview(attachment.agentAttachment)
        .then((url) => {
          if (active && url) {
            previewCache.current = new Map(previewCache.current).set(attachment.id, url)
            setPreviews(previewCache.current)
          }
        })
        .catch(() => {})
    }
    return () => {
      active = false
    }
    // URLs are bounded image derivatives and stay ephemeral: never write them into a draft.
  }, [attachments])
  return useMemo(
    () =>
      attachments.map((attachment) => ({
        ...attachment,
        previewUrl: attachment.previewUrl ?? previews.get(attachment.id)
      })),
    [attachments, previews]
  )
}
