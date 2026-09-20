import type { AgentInputAttachment, AttachmentImportProgress } from '@mycopilot/protocol'
import { hostClient } from '../../host/hostClient'

export type ComposerAttachmentKind = 'file' | 'image'

const IMPORT_CHUNK_BYTES = 512 * 1024

const IMAGE_EXTENSIONS = new Set(['gif', 'jpg', 'jpeg', 'png', 'webp'])

const READABLE_FILE_EXTENSIONS = new Set([
  'pdf',
  'docx',
  'doc',
  'pptx',
  'xlsx',
  'csv',
  'tsv',
  'txt',
  'text',
  'md',
  'markdown',
  'mdx',
  'rst',
  'log',
  'json',
  'jsonl',
  'yaml',
  'yml',
  'toml',
  'ini',
  'cfg',
  'conf',
  'env',
  'lock',
  'properties',
  'plist',
  'rc',
  'gitignore',
  'gitattributes',
  'editorconfig',
  'py',
  'pyi',
  'ipynb',
  'js',
  'jsx',
  'ts',
  'tsx',
  'mjs',
  'cjs',
  'd.ts',
  'html',
  'htm',
  'css',
  'scss',
  'sass',
  'less',
  'xml',
  'sql',
  'graphql',
  'gql',
  'proto',
  'prisma',
  'sh',
  'bash',
  'zsh',
  'fish',
  'ps1',
  'bat',
  'cmd',
  'rs',
  'go',
  'java',
  'kt',
  'kts',
  'c',
  'h',
  'cpp',
  'cc',
  'cxx',
  'hpp',
  'hh',
  'hxx',
  'cs',
  'php',
  'rb',
  'swift',
  'scala',
  'r',
  'm',
  'pl',
  'pm',
  'lua',
  'dart',
  'ex',
  'exs',
  'erl',
  'hrl',
  'clj',
  'cljs',
  'cljc',
  'edn',
  'fs',
  'fsi',
  'fsx',
  'elm',
  'hs',
  'lhs',
  'jl',
  'ml',
  'mli',
  'nim',
  'nims',
  'zig',
  'v',
  'vh',
  'sv',
  'svh',
  'sol',
  'tf',
  'tfvars',
  'hcl',
  'gradle',
  'groovy',
  'dockerfile',
  'cmake',
  'make',
  'mk',
  'tex',
  'bib',
  'vue',
  'svelte',
  'astro'
])

export interface ComposerAttachment {
  id: string
  kind: ComposerAttachmentKind
  name: string
  mimeType?: string
  sizeBytes: number
  previewUrl?: string
  agentAttachment: AgentInputAttachment
}

export function createAttachmentSummary(attachments: Array<{ name: string }>): string {
  if (attachments.length === 0) return ''
  return `Attachments: ${attachments.map((attachment) => attachment.name).join(', ')}`
}

export function stripAttachmentSummary(
  content: string,
  attachments: Array<{ name: string }>
): string {
  if (attachments.length === 0) return content

  const names = attachments.map((attachment) => attachment.name)
  const summaries = [createAttachmentSummary(attachments), `附件：${names.join('、')}`]
  for (const summary of summaries) {
    if (content === summary) return ''
    const suffix = `\n\n${summary}`
    if (content.endsWith(suffix)) return content.slice(0, -suffix.length)
  }
  return content
}

function getAttachmentsHost() {
  const attachmentsHost = (hostClient as Partial<Pick<typeof hostClient, 'attachments'>>)
    .attachments
  if (!attachmentsHost) {
    throw new Error('附件上传能力未加载，请重启应用后再试。')
  }
  return attachmentsHost
}

export async function selectComposerAttachments(
  kind: ComposerAttachmentKind,
  requestId?: string
): Promise<ComposerAttachment[]> {
  const attachments = await getAttachmentsHost().selectInputAttachments({
    kind,
    ...(requestId ? { requestId } : {})
  })
  return attachments.map(composerAttachmentFromAgentAttachment)
}

export interface AttachmentImportOptions {
  signal?: AbortSignal
  onProgress?: (progress: AttachmentImportProgress) => void
  onImported?: (attachment: ComposerAttachment) => void
  id?: string
}

export async function createComposerAttachmentsFromFiles(
  files: FileList | File[],
  options: AttachmentImportOptions = {}
): Promise<ComposerAttachment[]> {
  const fileList = Array.from(files)
  const attachments: ComposerAttachment[] = []

  for (const file of fileList) {
    const attachment = await createComposerAttachmentFromFile(file, options)
    attachments.push(attachment)
    options.onImported?.(attachment)
  }

  return attachments
}

export function composerAttachmentFromAgentAttachment(
  attachment: AgentInputAttachment
): ComposerAttachment {
  return {
    id: attachment.id,
    kind: attachment.kind,
    name: attachment.name,
    mimeType: attachment.mimeType,
    sizeBytes: attachment.sizeBytes,
    agentAttachment: attachment
  }
}

export function buildAgentInputAttachments(
  attachments: ComposerAttachment[]
): AgentInputAttachment[] {
  return attachments.map((attachment) => attachment.agentAttachment)
}

async function createComposerAttachmentFromFile(
  file: File,
  options: AttachmentImportOptions
): Promise<ComposerAttachment> {
  const kind = inferAttachmentKind(file)
  if (!kind) throw new Error(`不支持的附件类型：${file.name || file.type || '未知文件'}`)
  const metadata = {
    id: options.id ?? createAttachmentId(),
    kind,
    name: file.name || (kind === 'image' ? 'image' : 'attachment'),
    mimeType: file.type || inferMimeType(file.name, kind),
    sizeBytes: file.size
  }
  const host = getAttachmentsHost()
  let importId: string | undefined
  let receivedBytes = 0
  const checkCancelled = () => {
    if (options.signal?.aborted) throw new DOMException('附件导入已取消。', 'AbortError')
  }
  try {
    checkCancelled()
    options.onProgress?.({ attachment: metadata, receivedBytes, status: 'importing' })
    importId = (await host.beginImport(metadata)).importId
    while (receivedBytes < file.size) {
      checkCancelled()
      const bytes = new Uint8Array(
        await file.slice(receivedBytes, receivedBytes + IMPORT_CHUNK_BYTES).arrayBuffer()
      )
      const data = encodeBase64(bytes)
      const result = await host.appendImport({ importId, offset: receivedBytes, data })
      if (result.receivedBytes !== receivedBytes + bytes.byteLength)
        throw new Error('附件导入进度不一致，请重试。')
      receivedBytes = result.receivedBytes
      options.onProgress?.({ attachment: metadata, receivedBytes, status: 'importing' })
    }
    checkCancelled()
    const attachment = await host.finishImport({ importId })
    checkCancelled()
    options.onProgress?.({ attachment: metadata, receivedBytes, status: 'complete' })
    return composerAttachmentFromAgentAttachment(attachment)
  } catch (error) {
    if (importId) await host.cancelImport({ importId }).catch(() => undefined)
    options.onProgress?.({
      attachment: metadata,
      receivedBytes,
      status: options.signal?.aborted ? 'cancelled' : 'failed',
      error: error instanceof Error ? error.message : String(error)
    })
    throw error
  }
}

function encodeBase64(bytes: Uint8Array): string {
  let binary = ''
  for (let offset = 0; offset < bytes.length; offset += 32768) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 32768))
  }
  return btoa(binary)
}

const previewCache = new Map<string, Promise<string | undefined>>()

export async function loadComposerAttachmentPreview(
  attachment: AgentInputAttachment
): Promise<string | undefined> {
  if (attachment.kind !== 'image' || attachment.encoding !== 'managed') return undefined
  const key = `${attachment.id}:${attachment.data}`
  const cached = previewCache.get(key)
  if (cached) return cached
  const pending = getAttachmentsHost()
    .loadPreview({ attachment })
    .then((preview) =>
      preview?.mimeType.startsWith('image/')
        ? `data:${preview.mimeType};base64,${preview.data}`
        : undefined
    )
    .catch(() => {
      previewCache.delete(key)
      return undefined
    })
  if (previewCache.size >= 128) previewCache.delete(previewCache.keys().next().value as string)
  previewCache.set(key, pending)
  return pending
}

export async function loadComposerAttachmentImage(
  attachment: AgentInputAttachment
): Promise<string | undefined> {
  if (attachment.kind !== 'image' || attachment.encoding !== 'managed') return undefined
  const preview = await getAttachmentsHost().loadPreview({ attachment, purpose: 'display' })
  return preview?.mimeType.startsWith('image/')
    ? `data:${preview.mimeType};base64,${preview.data}`
    : undefined
}

function inferAttachmentKind(file: File): ComposerAttachmentKind | null {
  if (file.type.startsWith('image/'))
    return /^(image\/(png|jpeg|gif|webp))$/.test(file.type) ? 'image' : null

  const extension = fileExtension(file.name)
  if (IMAGE_EXTENSIONS.has(extension)) return 'image'
  if (file.type.startsWith('text/')) return 'file'
  if (READABLE_FILE_EXTENSIONS.has(extension)) return 'file'

  return null
}

function inferMimeType(name: string, kind: ComposerAttachmentKind): string {
  const extension = fileExtension(name)
  if (kind === 'image') {
    if (extension === 'jpg') return 'image/jpeg'
    if (extension) return `image/${extension}`
    return 'application/octet-stream'
  }

  if (extension === 'pdf') return 'application/pdf'
  if (extension === 'doc') return 'application/msword'
  if (extension === 'docx')
    return 'application/vnd.openxmlformats-officedocument.wordprocessingml.document'
  if (extension === 'pptx')
    return 'application/vnd.openxmlformats-officedocument.presentationml.presentation'
  if (extension === 'xlsx')
    return 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet'
  if (extension === 'csv') return 'text/csv'
  if (extension === 'tsv') return 'text/tab-separated-values'
  if (extension === 'html' || extension === 'htm') return 'text/html'
  if (extension === 'json' || extension === 'jsonl') return 'application/json'
  if (extension === 'xml') return 'application/xml'
  return 'text/plain'
}

function fileExtension(name: string): string {
  const normalizedName = name.trim().toLowerCase()
  if (!normalizedName.includes('.')) return ''
  return normalizedName.split('.').pop() ?? ''
}

function createAttachmentId(): string {
  return `attachment-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
}
