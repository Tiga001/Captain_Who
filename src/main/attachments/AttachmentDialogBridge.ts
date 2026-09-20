import { randomUUID } from 'node:crypto'
import { open } from 'node:fs/promises'
import { basename, extname } from 'node:path'
import { BrowserWindow, dialog } from 'electron'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import type { CoreServer } from '../core/coreServer'
import type { IpcMainInvokeEvent, OpenDialogOptions } from 'electron'
import type {
  AgentInputAttachment,
  AttachmentImportMetadata,
  AttachmentImportProgress,
  AttachmentSelectInputRequest,
  AttachmentSelectionKind
} from '@mycopilot/protocol'

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
  csv: 'text/csv',
  doc: 'application/msword',
  docx: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
  gif: 'image/gif',
  htm: 'text/html',
  html: 'text/html',
  jpeg: 'image/jpeg',
  jpg: 'image/jpeg',
  json: 'application/json',
  jsonl: 'application/json',
  pdf: 'application/pdf',
  png: 'image/png',
  pptx: 'application/vnd.openxmlformats-officedocument.presentationml.presentation',
  tsv: 'text/tab-separated-values',
  txt: 'text/plain',
  webp: 'image/webp',
  xlsx: 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
  xml: 'application/xml',
  yaml: 'text/yaml',
  yml: 'text/yaml'
}

type ImportBackend = Pick<
  CoreServer,
  | 'beginAttachmentImport'
  | 'appendAttachmentImport'
  | 'finishAttachmentImport'
  | 'cancelAttachmentImport'
>

interface SelectedImport {
  requestId?: string
  path: string
  event: IpcMainInvokeEvent
  metadata: AttachmentImportMetadata
  cancelled: boolean
  importId?: string
  running: boolean
}

export class AttachmentDialogBridge {
  private readonly selections = new Map<string, SelectedImport>()
  private readonly owners = new Set<number>()
  private readonly requests = new Map<string, { owner: number; cancelled: boolean }>()

  constructor(private readonly backend: ImportBackend) {}

  async selectInputAttachments(
    event: IpcMainInvokeEvent,
    request: AttachmentSelectInputRequest
  ): Promise<AgentInputAttachment[]> {
    if (request.kind !== 'file' && request.kind !== 'image')
      throw new Error('Invalid attachment kind')
    if (request.requestId && this.requests.has(request.requestId))
      throw new Error('Attachment request already exists')
    const batch = { owner: event.sender.id, cancelled: false }
    if (request.requestId) this.requests.set(request.requestId, batch)
    try {
      const window = BrowserWindow.fromWebContents(event.sender) ?? undefined
      const options = dialogOptionsForKind(request.kind)
      const result = window
        ? await dialog.showOpenDialog(window, options)
        : await dialog.showOpenDialog(options)
      if (result.canceled || batch.cancelled || event.sender.isDestroyed()) return []
      if (!this.owners.has(event.sender.id)) {
        this.owners.add(event.sender.id)
        event.sender.once('destroyed', () => {
          this.owners.delete(event.sender.id)
          for (const batch of this.requests.values()) {
            if (batch.owner === event.sender.id) batch.cancelled = true
          }
          for (const [id, selection] of this.selections) {
            if (selection.event.sender.id === event.sender.id) {
              selection.cancelled = true
              this.selections.delete(id)
            }
          }
        })
      }
      const attachments: AgentInputAttachment[] = []
      for (const path of result.filePaths) {
        if (batch.cancelled || event.sender.isDestroyed()) break
        const extension = extensionForPath(path)
        const kind = inferAttachmentKind(extension) ?? 'file'
        const metadata: AttachmentImportMetadata = {
          id: `attachment-${randomUUID()}`,
          kind,
          name: basename(path),
          mimeType: mimeTypeForExtension(extension, kind),
          sizeBytes: 0
        }
        const selection: SelectedImport = {
          path,
          event,
          metadata,
          requestId: request.requestId,
          cancelled: false,
          running: false
        }
        this.selections.set(metadata.id, selection)
        // The grant belongs to this window and only to paths returned by the native picker.
        try {
          attachments.push(await this.importSelection(selection))
        } catch {
          /* each failed card retains its retry grant */
        }
      }
      return attachments
    } finally {
      if (request.requestId) this.requests.delete(request.requestId)
    }
  }

  async cancelImport(event: IpcMainInvokeEvent, id: string): Promise<void> {
    const batch = this.requests.get(id)
    if (batch) {
      if (batch.owner !== event.sender.id) throw new Error('Attachment import owner mismatch')
      batch.cancelled = true
      for (const selection of this.selections.values()) {
        if (selection.requestId === id) selection.cancelled = true
      }
      return
    }
    const matching = [...this.selections.values()].filter((selection) => selection.requestId === id)
    if (matching.length) {
      if (matching.some((selection) => selection.event.sender.id !== event.sender.id))
        throw new Error('Attachment import owner mismatch')
      for (const selection of matching) selection.cancelled = true
      return
    }
    const selection = this.selections.get(id)
    if (!selection) {
      await this.backend.cancelAttachmentImport({ importId: id })
      return
    }
    if (selection.event.sender.id !== event.sender.id)
      throw new Error('Attachment import owner mismatch')
    selection.cancelled = true
    if (!selection.running) this.selections.delete(id)
  }

  async retryInputAttachment(
    event: IpcMainInvokeEvent,
    id: string,
    requestId?: string
  ): Promise<AgentInputAttachment> {
    const selection = this.selections.get(id)
    if (!selection || selection.event.sender.id !== event.sender.id || selection.running) {
      throw new Error('附件已不可重试，请重新选择。')
    }
    selection.cancelled = false
    selection.requestId = requestId
    return this.importSelection(selection)
  }

  private progress(
    selection: SelectedImport,
    progress: Omit<AttachmentImportProgress, 'attachment'>
  ): void {
    if (!selection.event.sender.isDestroyed()) {
      selection.event.sender.send(HOST_CHANNELS.attachments.importProgress, {
        attachment: selection.metadata,
        requestId: selection.requestId,
        ...progress
      } satisfies AttachmentImportProgress)
    }
  }

  private async importSelection(selection: SelectedImport): Promise<AgentInputAttachment> {
    selection.running = true
    let receivedBytes = 0
    let handle: Awaited<ReturnType<typeof open>> | undefined
    this.progress(selection, { receivedBytes, status: 'importing' })
    try {
      if (selection.cancelled) throw new Error('附件导入已取消。')
      if (!inferAttachmentKind(extensionForPath(selection.path)))
        throw new Error(`不支持的附件类型：${selection.metadata.name}`)
      handle = await open(selection.path, 'r')
      const before = await handle.stat()
      if (!before.isFile()) throw new Error('请选择文件作为附件。')
      selection.metadata.sizeBytes = before.size
      this.progress(selection, { receivedBytes, status: 'importing' })
      const { importId } = await this.backend.beginAttachmentImport(selection.metadata)
      selection.importId = importId
      const buffer = Buffer.allocUnsafe(IMPORT_CHUNK_BYTES)
      while (receivedBytes < before.size) {
        if (selection.cancelled || selection.event.sender.isDestroyed())
          throw new Error('附件导入已取消。')
        const { bytesRead } = await handle.read(
          buffer,
          0,
          Math.min(buffer.length, before.size - receivedBytes),
          receivedBytes
        )
        if (bytesRead === 0) throw new Error('附件读取不完整，请重新选择。')
        const response = await this.backend.appendAttachmentImport({
          importId,
          offset: receivedBytes,
          data: buffer.subarray(0, bytesRead).toString('base64')
        })
        if (response.receivedBytes !== receivedBytes + bytesRead) {
          throw new Error('附件导入进度不一致，请重试。')
        }
        receivedBytes = response.receivedBytes
        this.progress(selection, { receivedBytes, status: 'importing' })
      }
      const after = await handle.stat()
      if (selection.cancelled || after.size !== before.size || after.mtimeMs !== before.mtimeMs) {
        throw new Error(selection.cancelled ? '附件导入已取消。' : '附件已发生变化，请重新选择。')
      }
      const attachment = await this.backend.finishAttachmentImport({ importId })
      if (selection.cancelled || selection.event.sender.isDestroyed())
        throw new Error('附件导入已取消。')
      this.selections.delete(selection.metadata.id)
      this.progress(selection, { receivedBytes, status: 'complete' })
      return attachment
    } catch (error) {
      if (selection.importId)
        await this.backend
          .cancelAttachmentImport({ importId: selection.importId })
          .catch(() => undefined)
      this.progress(selection, {
        receivedBytes,
        status: selection.cancelled ? 'cancelled' : 'failed',
        error: String(error instanceof Error ? error.message : error)
      })
      if (selection.cancelled) this.selections.delete(selection.metadata.id)
      throw error
    } finally {
      selection.running = false
      selection.importId = undefined
      await handle?.close()
    }
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
