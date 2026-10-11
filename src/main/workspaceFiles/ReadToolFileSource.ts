import type {
  AgentEvent,
  AgentObserverEventEnvelope,
  ReadToolFileRequest,
  StorageReadToolFileContext,
  StorageReadToolFileContextRequest
} from '@mycopilot/protocol'
import { resolveProjectFileReference } from '../projects/projectFilePaths'

interface Source {
  loadReadToolFileContext(
    input: StorageReadToolFileContextRequest
  ): Promise<StorageReadToolFileContext | null>
  loadProjects: Parameters<typeof resolveProjectFileReference>[0]['loadProjects']
  resolveRunWorkspacePath: Parameters<
    typeof resolveProjectFileReference
  >[0]['resolveRunWorkspacePath']
  resolveRunAttachmentFile(input: {
    conversationId: string
    assistantMessageId: string
    filePath: string
  }): Promise<{
    path: string
    name: string
  }>
  onAgentEvent?(handler: (event: AgentEvent) => void): () => void
  onCollaborationObserverEvent?(handler: (event: AgentObserverEventEnvelope) => void): () => void
  onStarted?(handler: () => void): () => void
}

type RecordValue = Record<string, unknown>
const READ_TOOLS = new Set(['read_file', 'read_word', 'read_spreadsheet', 'read_presentation'])
const unavailable = () => new Error('The referenced read file is not available.')
const record = (value: unknown): RecordValue =>
  value && typeof value === 'object' && !Array.isArray(value) ? (value as RecordValue) : {}
const text = (value: unknown): string => (typeof value === 'string' ? value.trim() : '')

interface LiveRun {
  conversationId?: string
  assistantMessageId?: string
  calls: Map<string, RecordValue>
  results: Map<string, RecordValue>
}

/** Keeps live read-call proof in the Host while the durable renderer projection catches up. */
export class ReadToolFileSource {
  private readonly live = new Map<string, LiveRun>()
  private readonly disposers: Array<() => void> = []

  constructor(private readonly source: Source) {
    const legacy = source.onAgentEvent?.((event) => this.observe(event))
    const observer = source.onCollaborationObserverEvent?.((envelope) =>
      this.observe(envelope.event, envelope)
    )
    const restart = source.onStarted?.(() => this.live.clear())
    this.disposers.push(...[legacy, observer, restart].filter((value) => value !== undefined))
  }

  dispose(): void {
    this.disposers.forEach((dispose) => dispose())
    this.live.clear()
  }

  private observe(event: AgentEvent, identity?: AgentObserverEventEnvelope): void {
    if (event.type !== 'tool_call' && event.type !== 'tool_result') return
    if (
      event.type === 'tool_call' &&
      (event.identity.type !== 'builtin' || event.identity.toolName !== event.call.tool)
    )
      return
    const run: LiveRun = this.live.get(event.runId) ?? { calls: new Map(), results: new Map() }
    if (identity) {
      run.conversationId = identity.conversationId
      run.assistantMessageId = identity.assistantMessageId
    }
    if (event.type === 'tool_call' && READ_TOOLS.has(event.call.tool)) {
      const args = record(event.call.args)
      run.calls.set(event.call.id, {
        id: event.call.id,
        tool: event.call.tool,
        args: { path: args.path, filePath: args.filePath }
      })
    } else if (event.type === 'tool_result') {
      if (READ_TOOLS.has(event.result.tool)) {
        const result = record(event.result.result)
        run.results.set(event.result.callId, {
          callId: event.result.callId,
          tool: event.result.tool,
          result: { path: result.path }
        })
      }
    }
    if (!run.calls.size && !run.results.size) return
    this.live.set(event.runId, run)
    while (run.calls.size > 512) run.calls.delete(run.calls.keys().next().value!)
    while (run.results.size > 512) run.results.delete(run.results.keys().next().value!)
    while (this.live.size > 128) this.live.delete(this.live.keys().next().value!)
  }

  async resolveReadToolFile(input: ReadToolFileRequest): Promise<{ path: string; name?: string }> {
    if (
      input.source !== 'read-tool' ||
      !text(input.conversationId) ||
      !text(input.assistantMessageId) ||
      !text(input.callId) ||
      !text(input.filePath) ||
      /[\0\r\n]/.test(input.filePath)
    )
      throw unavailable()
    const stored = await this.source.loadReadToolFileContext({
      conversationId: input.conversationId,
      assistantMessageId: input.assistantMessageId,
      callId: input.callId
    })
    if (
      !stored ||
      stored.conversationId !== input.conversationId ||
      stored.assistantMessageId !== input.assistantMessageId
    )
      throw unavailable()
    if (stored.projectId !== (input.projectId ?? null)) throw unavailable()
    const live = [...this.live.entries()].find(
      ([runId, run]) =>
        (run.conversationId === input.conversationId &&
          run.assistantMessageId === input.assistantMessageId) ||
        (stored.messageFound && runId === stored.runId)
    )?.[1]
    if (!stored.messageFound && !live) throw unavailable()
    const call =
      live?.calls.get(input.callId) ??
      (stored.call?.id === input.callId
        ? {
            id: stored.call.id,
            tool: stored.call.tool,
            args: { path: stored.call.path, filePath: stored.call.filePath }
          }
        : undefined)
    if (!call || !READ_TOOLS.has(text(call.tool))) throw unavailable()
    if (
      stored.callIdentity !== null &&
      (stored.callIdentity.type !== 'builtin' || stored.callIdentity.toolName !== call.tool)
    )
      throw unavailable()
    const liveResult = live?.results.get(input.callId)
    const args = record(call.args)
    const filePath = input.filePath.trim()
    const knownPaths = [
      text(liveResult ? record(liveResult.result).path : stored.resultPath),
      text(args.path) || text(args.filePath)
    ]
    if (!knownPaths.some((path) => path && path === filePath)) throw unavailable()

    if (filePath.startsWith('@attachments/')) {
      const attachment = await this.source.resolveRunAttachmentFile({
        conversationId: input.conversationId,
        assistantMessageId: input.assistantMessageId,
        filePath
      })
      return { path: attachment.path, name: attachment.name }
    }
    // Managed artifact/download identifiers need their own ownership-aware resolver.
    if (/^[a-z][a-z\d+.-]*:/i.test(filePath) && !/^[a-z]:[\\/]/i.test(filePath)) {
      throw unavailable()
    }
    return {
      path: await resolveProjectFileReference(this.source, {
        projectId: stored.projectId,
        assistantMessageId: input.assistantMessageId,
        filePath
      })
    }
  }
}
