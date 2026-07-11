import { randomUUID } from 'node:crypto'
import { readFile, stat } from 'node:fs/promises'
import { basename, extname } from 'node:path'
import { BrowserWindow, dialog } from 'electron'
import type { IpcMainInvokeEvent, OpenDialogOptions, OpenDialogReturnValue } from 'electron'
import type {
  AgentInputAttachment,
  AttachmentLoadFromPathsRequest,
  AttachmentSelectInputRequest,
  AttachmentSelectionKind
} from '@mycopilot/protocol'

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

const MIME_BY_EXTENSION: Record<string, string> = {
  avif: 'image/avif',
  bmp: 'image/bmp',
  csv: 'text/csv',
  doc: 'application/msword',
  docx: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
  gif: 'image/gif',
  htm: 'text/html',
  html: 'text/html',
  ico: 'image/x-icon',
  jpeg: 'image/jpeg',
  jpg: 'image/jpeg',
  json: 'application/json',
  jsonl: 'application/json',
  pdf: 'application/pdf',
  png: 'image/png',
  pptx: 'application/vnd.openxmlformats-officedocument.presentationml.presentation',
  svg: 'image/svg+xml',
  tif: 'image/tiff',
  tiff: 'image/tiff',
  tsv: 'text/tab-separated-values',
  txt: 'text/plain',
  webp: 'image/webp',
  xlsx: 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
  xml: 'application/xml',
  yaml: 'text/yaml',
  yml: 'text/yaml'
}

export class AttachmentDialogBridge {
  async selectInputAttachments(
    event: IpcMainInvokeEvent,
    request: AttachmentSelectInputRequest
  ): Promise<AgentInputAttachment[]> {
    const result = await this.showOpenDialog(event, dialogOptionsForKind(request.kind))
    if (result.canceled || result.filePaths.length === 0) {
      return []
    }

    return this.loadInputAttachmentsFromPaths({ paths: result.filePaths })
  }

  async loadInputAttachmentsFromPaths(
    request: AttachmentLoadFromPathsRequest
  ): Promise<AgentInputAttachment[]> {
    const attachments: AgentInputAttachment[] = []

    for (const filePath of request.paths) {
      attachments.push(await attachmentFromPath(filePath))
    }

    return attachments
  }

  private showOpenDialog(
    event: IpcMainInvokeEvent,
    options: OpenDialogOptions
  ): Promise<OpenDialogReturnValue> {
    const window = BrowserWindow.fromWebContents(event.sender) ?? undefined
    return window ? dialog.showOpenDialog(window, options) : dialog.showOpenDialog(options)
  }
}

function dialogOptionsForKind(kind: AttachmentSelectionKind): OpenDialogOptions {
  if (kind === 'image') {
    return {
      title: 'Select images',
      properties: ['openFile', 'multiSelections'],
      filters: [{ name: 'Images', extensions: [...IMAGE_EXTENSIONS].sort() }]
    }
  }

  return {
    title: 'Select files',
    properties: ['openFile', 'multiSelections'],
    filters: [
      { name: 'Readable files', extensions: [...READABLE_FILE_EXTENSIONS].sort() },
      { name: 'Images', extensions: [...IMAGE_EXTENSIONS].sort() }
    ]
  }
}

async function attachmentFromPath(filePath: string): Promise<AgentInputAttachment> {
  const metadata = await stat(filePath).catch((error) => {
    throw new Error(`附件文件不可访问：${error}`)
  })
  if (!metadata.isFile()) {
    throw new Error('请选择文件作为附件。')
  }
  if (metadata.size > MAX_ATTACHMENT_BYTES) {
    throw new Error(`附件「${basename(filePath)}」过大，单个附件最大 8 MB。`)
  }

  const extension = extensionForPath(filePath)
  const kind = inferAttachmentKind(extension)
  if (!kind) {
    throw new Error(`不支持的附件类型：${basename(filePath) || '未知文件'}`)
  }

  const bytes = await readFile(filePath).catch((error) => {
    throw new Error(`读取附件「${basename(filePath)}」失败：${error}`)
  })

  return {
    id: `attachment-${randomUUID()}`,
    kind,
    name: basename(filePath) || (kind === 'image' ? 'image' : 'attachment'),
    mimeType: mimeTypeForExtension(extension, kind),
    sizeBytes: metadata.size,
    encoding: 'base64',
    data: bytes.toString('base64')
  }
}

function inferAttachmentKind(extension: string): AgentInputAttachment['kind'] | null {
  if (IMAGE_EXTENSIONS.has(extension)) return 'image'
  if (READABLE_FILE_EXTENSIONS.has(extension)) return 'file'
  return null
}

function mimeTypeForExtension(extension: string, kind: AgentInputAttachment['kind']): string {
  const mapped = MIME_BY_EXTENSION[extension]
  if (mapped) return mapped
  if (kind === 'image') return extension ? `image/${extension}` : 'application/octet-stream'
  return 'text/plain'
}

function extensionForPath(filePath: string): string {
  return extname(filePath).replace(/^\./, '').toLowerCase()
}
