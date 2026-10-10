import { randomUUID } from 'node:crypto'
import { constants, renameSync } from 'node:fs'
import { open, rm } from 'node:fs/promises'
import { dirname, extname, join } from 'node:path'
import { captureHostInvocation, HOST_CHANNELS } from '@mycopilot/host-api'
import { parseWorkflowRequest, WORKFLOW_TEMPLATE_MARKDOWN_MAX_BYTES } from '@mycopilot/protocol'
import type { IpcMainInvokeEvent } from 'electron'
import type { CoreServer } from '../core/coreServer'
import type { TrustedIpcMain } from './trustedIpc'

export interface WorkflowTemplateSession {
  assertCurrent(): void
  dispose(): void
}

export interface WorkflowTemplateIpcDependencies {
  selectImportPath(event: IpcMainInvokeEvent): Promise<string | null>
  selectExportPath(event: IpcMainInvokeEvent, suggestedFileName: string): Promise<string | null>
  beginSession(event: IpcMainInvokeEvent): WorkflowTemplateSession
}

class WorkflowTemplateFileError extends Error {
  readonly data: { code: string }

  constructor(code: string) {
    super(code)
    this.data = { code }
  }
}

function fileError(suffix: string): WorkflowTemplateFileError {
  return new WorkflowTemplateFileError(`organization_template_${suffix}`)
}

/** Bounded even if the selected file grows after stat; decoding never replaces malformed UTF-8. */
async function readTemplateMarkdown(path: string): Promise<string> {
  if (extname(path).toLowerCase() !== '.md') throw fileError('invalid_format')
  try {
    const handle = await open(path, constants.O_RDONLY | constants.O_NONBLOCK)
    try {
      const stat = await handle.stat()
      if (!stat.isFile()) throw fileError('invalid_format')
      if (stat.size > WORKFLOW_TEMPLATE_MARKDOWN_MAX_BYTES) throw fileError('too_large')
      const bytes = Buffer.alloc(WORKFLOW_TEMPLATE_MARKDOWN_MAX_BYTES + 1)
      let size = 0
      while (size < bytes.length) {
        const { bytesRead } = await handle.read(bytes, size, bytes.length - size, size)
        if (!bytesRead) break
        size += bytesRead
      }
      if (size > WORKFLOW_TEMPLATE_MARKDOWN_MAX_BYTES) throw fileError('too_large')
      try {
        const markdown = new TextDecoder('utf-8', { fatal: true }).decode(bytes.subarray(0, size))
        if (!markdown.trim()) throw fileError('invalid_format')
        return markdown
      } catch {
        throw fileError('invalid_format')
      }
    } finally {
      await handle.close()
    }
  } catch (error) {
    if (error instanceof WorkflowTemplateFileError) throw error
    throw fileError('read_failed')
  }
}

/** Stage in the destination directory; the session fence and atomic publication cannot interleave. */
async function writeTemplateMarkdown(
  path: string,
  markdown: string,
  session: WorkflowTemplateSession
): Promise<void> {
  const temporaryPath = join(dirname(path), `.organization-template-${randomUUID()}.tmp`)
  let staged = false
  try {
    try {
      const handle = await open(temporaryPath, 'wx', 0o600)
      staged = true
      try {
        await handle.writeFile(markdown, 'utf8')
        await handle.sync()
      } finally {
        await handle.close()
      }
    } catch {
      throw fileError('write_failed')
    }
    session.assertCurrent()
    try {
      renameSync(temporaryPath, path)
      staged = false
    } catch {
      throw fileError('write_failed')
    }
  } finally {
    if (staged) await rm(temporaryPath, { force: true }).catch(() => undefined)
  }
}

export function registerWorkflowTemplateIpc(
  ipcMain: TrustedIpcMain,
  coreServer: Pick<CoreServer, 'requestWorkflows'>,
  dependencies: WorkflowTemplateIpcDependencies
): void {
  ipcMain.handle(HOST_CHANNELS.agent.workflowTemplateImport, (event, ...args) =>
    captureHostInvocation(async () => {
      if (args.length) throw fileError('invalid_request')
      const session = dependencies.beginSession(event)
      try {
        session.assertCurrent()
        let source: string | null
        try {
          source = await dependencies.selectImportPath(event)
        } catch {
          throw fileError('dialog_failed')
        }
        if (source === null) return null
        session.assertCurrent()
        const markdown = await readTemplateMarkdown(source)
        session.assertCurrent()
        const response = await coreServer.requestWorkflows({
          operation: 'importTemplateMarkdown',
          markdown
        })
        session.assertCurrent()
        return response
      } finally {
        session.dispose()
      }
    })
  )
  ipcMain.handle(HOST_CHANNELS.agent.workflowTemplateExport, (event, value) =>
    captureHostInvocation(async () => {
      let input: Extract<
        ReturnType<typeof parseWorkflowRequest>,
        { operation: 'exportTemplateMarkdown' }
      >
      try {
        if (
          !value ||
          typeof value !== 'object' ||
          Array.isArray(value) ||
          Object.keys(value).some((key) => !['id', 'expectedRevision', 'language'].includes(key))
        ) {
          throw fileError('invalid_request')
        }
        const request = parseWorkflowRequest({ ...value, operation: 'exportTemplateMarkdown' })
        if (request.operation !== 'exportTemplateMarkdown') throw fileError('invalid_request')
        input = request
      } catch {
        throw fileError('invalid_request')
      }
      const session = dependencies.beginSession(event)
      try {
        session.assertCurrent()
        const response = await coreServer.requestWorkflows(input)
        session.assertCurrent()
        const exported = response.exportedTemplate
        if (!exported) throw fileError('invalid_format')
        let destination: string | null
        try {
          destination = await dependencies.selectExportPath(event, exported.suggestedFileName)
        } catch {
          throw fileError('dialog_failed')
        }
        if (destination === null) return { saved: false }
        session.assertCurrent()
        await writeTemplateMarkdown(destination, exported.markdown, session)
        return { saved: true }
      } finally {
        session.dispose()
      }
    })
  )
}
