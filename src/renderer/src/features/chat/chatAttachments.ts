import type { AgentInputAttachment } from '@mycopilot/protocol'
import { hostClient } from '../../host/hostClient'

export type ComposerAttachmentKind = 'file' | 'image'

const MAX_ATTACHMENT_BYTES = 8 * 1024 * 1024

const IMAGE_EXTENSIONS = new Set([
  'apng',
  'avif',
  'bmp',
  'gif',
  'heic',
  'heif',
  'ico',
  'jpg',
  'jpeg',
  'png',
  'svg',
  'tif',
  'tiff',
  'webp'
])

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
  kind: ComposerAttachmentKind
): Promise<ComposerAttachment[]> {
  const attachments = await getAttachmentsHost().selectInputAttachments({ kind })
  return attachments.map(composerAttachmentFromAgentAttachment)
}

export async function createComposerAttachmentsFromFiles(
  files: FileList | File[]
): Promise<ComposerAttachment[]> {
  const fileList = Array.from(files)
  const attachments: ComposerAttachment[] = []

  for (const file of fileList) {
    attachments.push(await createComposerAttachmentFromFile(file))
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
    previewUrl: previewUrlForAttachment(attachment),
    agentAttachment: attachment
  }
}

export function buildAgentInputAttachments(
  attachments: ComposerAttachment[]
): AgentInputAttachment[] {
  return attachments.map((attachment) => attachment.agentAttachment)
}

function createComposerAttachmentFromFile(file: File): Promise<ComposerAttachment> {
  const kind = inferAttachmentKind(file)
  if (!kind) {
    throw new Error(`不支持的附件类型：${file.name || file.type || '未知文件'}`)
  }

  if (file.size > MAX_ATTACHMENT_BYTES) {
    throw new Error(`附件「${file.name}」过大，单个附件最大 8 MB。`)
  }

  return readFileAsBase64(file).then((data) => {
    const attachment: AgentInputAttachment = {
      id: createAttachmentId(),
      kind,
      name: file.name || (kind === 'image' ? 'image' : 'attachment'),
      mimeType: file.type || inferMimeType(file.name, kind),
      sizeBytes: file.size,
      encoding: 'base64',
      data
    }

    return composerAttachmentFromAgentAttachment(attachment)
  })
}

function inferAttachmentKind(file: File): ComposerAttachmentKind | null {
  if (file.type.startsWith('image/')) return 'image'

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
    if (extension === 'svg') return 'image/svg+xml'
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
  if (extension === 'svg') return 'image/svg+xml'
  return 'text/plain'
}

function readFileAsBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onerror = () => reject(new Error(`读取附件「${file.name}」失败。`))
    reader.onload = () => {
      const result = String(reader.result ?? '')
      resolve(result.includes(',') ? result.split(',')[1] : result)
    }
    reader.readAsDataURL(file)
  })
}

function fileExtension(name: string): string {
  const normalizedName = name.trim().toLowerCase()
  if (!normalizedName.includes('.')) return ''
  return normalizedName.split('.').pop() ?? ''
}

function createAttachmentId(): string {
  return `attachment-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
}

function previewUrlForAttachment(attachment: AgentInputAttachment): string | undefined {
  if (attachment.kind !== 'image') return undefined
  if (attachment.encoding !== 'base64') return undefined
  if (!attachment.mimeType?.startsWith('image/')) return undefined
  return `data:${attachment.mimeType};base64,${attachment.data}`
}
