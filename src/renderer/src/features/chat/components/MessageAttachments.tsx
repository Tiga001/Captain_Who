import { useEffect, useMemo, useRef, useState } from 'react'
import type { AgentInputAttachment } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import type { ChatMessage } from '../chatTypes'
import { loadComposerAttachmentImage } from '../chatAttachments'
import { useComposerAttachmentPreviews } from '../useAttachmentImports'
import { getAttachmentPreviewUrl } from '../attachmentDisplay'
import { loadAttachmentImage } from '../../storage/storageClient'
import * as storageClient from '../../storage/storageClient'
import { ComposerAttachments, type ComposerAttachmentPresentation } from './ComposerAttachments'
import { useImagePreview, useImagePreviewNotice } from './ImagePreview'

function managedImageInput(
  attachment: NonNullable<ChatMessage['attachments']>[number]
): AgentInputAttachment | undefined {
  if (attachment.kind !== 'image' || attachment.encoding !== 'managed' || !attachment.data)
    return undefined
  return {
    id: attachment.id,
    kind: 'image',
    name: attachment.name,
    mimeType: attachment.mimeType ?? undefined,
    sizeBytes: attachment.sizeBytes,
    encoding: 'managed',
    data: attachment.data
  }
}

export function MessageAttachments({
  attachments,
  messageId,
  mode
}: {
  attachments?: ChatMessage['attachments']
  messageId: string
  mode: 'interactive' | 'observer'
}) {
  const { t } = useFrontendConfig()
  const openImagePreview = useImagePreview()
  const showImagePreviewNotice = useImagePreviewNotice()
  const [hydratedManagedAttachments, setHydratedManagedAttachments] = useState<
    Map<string, AgentInputAttachment>
  >(new Map())
  const hydrationRequests = useRef(new Set<string>())
  useEffect(() => {
    const candidates =
      mode === 'interactive'
        ? (attachments ?? []).filter(
            (attachment) =>
              attachment.kind === 'image' &&
              !getAttachmentPreviewUrl(attachment) &&
              !managedImageInput(attachment)
          )
        : []
    const candidateIds = new Set(candidates.map((attachment) => attachment.id))
    hydrationRequests.current = new Set(
      [...hydrationRequests.current].filter((id) => candidateIds.has(id))
    )
    setHydratedManagedAttachments((previous) => {
      const next = new Map([...previous].filter(([id]) => candidateIds.has(id)))
      return next.size === previous.size ? previous : next
    })
    if (candidates.length === 0) return

    const pending = candidates.filter((attachment) => !hydrationRequests.current.has(attachment.id))
    if (pending.length === 0) return
    if (typeof storageClient.loadInputAttachments !== 'function') return
    for (const attachment of pending) hydrationRequests.current.add(attachment.id)
    let active = true
    void Promise.allSettled(
      pending.map(async (attachment) => {
        const [managed] = await storageClient.loadInputAttachments([attachment.id])
        return managed
      })
    ).then((results) => {
      if (!active) return
      setHydratedManagedAttachments((previous) => {
        const next = new Map(previous)
        for (const result of results) {
          if (result.status === 'fulfilled' && result.value) {
            next.set(result.value.id, result.value)
          }
        }
        return next
      })
    })
    return () => {
      active = false
    }
  }, [attachments, mode])
  const previewInputs = useMemo(
    () =>
      (attachments ?? []).map((attachment) => ({
        id: attachment.id,
        kind: attachment.kind,
        previewUrl: getAttachmentPreviewUrl(attachment),
        agentAttachment:
          mode === 'interactive'
            ? (managedImageInput(attachment) ?? hydratedManagedAttachments.get(attachment.id))
            : undefined
      })),
    [attachments, hydratedManagedAttachments, mode]
  )
  const previewAttachments = useComposerAttachmentPreviews(previewInputs)
  const previewUrls = new Map(
    previewAttachments.map((attachment) => [attachment.id, attachment.previewUrl])
  )
  const previewRequestRef = useRef(0)
  useEffect(
    () => () => {
      previewRequestRef.current += 1
    },
    [messageId]
  )
  if (!attachments?.length) return null

  const openOriginalImageAttachment = async (
    attachment: NonNullable<ChatMessage['attachments']>[number]
  ) => {
    const previewRequest = ++previewRequestRef.current
    if (mode === 'observer') {
      // Observer DTOs carry only authorized display bytes, never an ordinary attachment input.
      const src = getAttachmentPreviewUrl(attachment)
      if (src) openImagePreview({ alt: attachment.name, fileName: attachment.name, src })
      return
    }

    try {
      const managed = managedImageInput(attachment) ?? hydratedManagedAttachments.get(attachment.id)
      if (managed) {
        const src = await loadComposerAttachmentImage(managed)
        if (previewRequestRef.current !== previewRequest) return
        if (src) {
          openImagePreview({ alt: attachment.name, fileName: attachment.name, src })
          return
        }
      }
      const image = await loadAttachmentImage(attachment.id)
      if (previewRequestRef.current !== previewRequest) return
      if (!image?.mimeType.startsWith('image/') || !image.data) {
        showImagePreviewNotice(t('imagePreview.originalMissing'))
        return
      }

      openImagePreview({
        alt: image.name || attachment.name,
        fileName: image.name || attachment.name,
        src: `data:${image.mimeType};base64,${image.data}`
      })
    } catch (error) {
      if (previewRequestRef.current !== previewRequest) return
      console.error('Failed to load attachment image', error)
      showImagePreviewNotice(t('imagePreview.originalMissing'))
    }
  }

  const displayAttachments: ComposerAttachmentPresentation[] = attachments.map((attachment) => ({
    id: attachment.id,
    kind: attachment.kind,
    name: attachment.name,
    sizeBytes: attachment.sizeBytes,
    previewUrl: getAttachmentPreviewUrl(attachment) ?? previewUrls.get(attachment.id)
  }))

  return (
    <ComposerAttachments
      attachments={displayAttachments}
      label={t('chat.attachments')}
      messageId={messageId}
      onPreviewAttachment={(id) => {
        const attachment = attachments.find((candidate) => candidate.id === id)
        if (attachment) void openOriginalImageAttachment(attachment)
      }}
      removeLabel=""
      variant="message"
    />
  )
}
