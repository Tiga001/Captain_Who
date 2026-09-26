interface AttachmentPreviewInput {
  kind: 'file' | 'image'
  previewData?: string | null
  previewMimeType?: string | null
}

export function getAttachmentPreviewUrl(attachment: AttachmentPreviewInput) {
  if (attachment.kind !== 'image') return undefined
  if (attachment.previewData && attachment.previewMimeType?.startsWith('image/')) {
    return `data:${attachment.previewMimeType};base64,${attachment.previewData}`
  }
  return undefined
}
