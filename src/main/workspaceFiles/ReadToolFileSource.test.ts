import { mkdtemp, mkdir, realpath, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import type {
  AgentEvent,
  AgentObserverEventEnvelope,
  ReadToolFileRequest,
  StorageChatConversationRecord
} from '@mycopilot/protocol'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ReadToolFileSource } from './ReadToolFileSource'
import { WorkspaceFilesService } from './WorkspaceFilesService'

describe('read tool file preview authority', () => {
  let root: string
  let filePath: string
  let stored: StorageChatConversationRecord
  let onEvent: (event: AgentEvent) => void
  let onObserver: (event: AgentObserverEventEnvelope) => void
  let onRestart: () => void
  let source: ReturnType<typeof createSource>
  let resolver: ReadToolFileSource
  let service: WorkspaceFilesService
  const request = (overrides: Partial<ReadToolFileRequest> = {}): ReadToolFileRequest => ({
    source: 'read-tool',
    conversationId: 'conversation',
    assistantMessageId: 'assistant',
    callId: 'read',
    filePath,
    projectId: 'project',
    ...overrides
  })
  function persistRead(path = filePath, tool = 'read_file') {
    stored.messages[0].agentRunJson = JSON.stringify({
      runId: 'run',
      toolCalls: [{ id: 'read', tool, args: { path } }],
      toolResults: []
    })
  }
  function createSource() {
    return {
      loadConversation: vi.fn(async () => stored),
      loadProjects: vi.fn(async () => []),
      resolveRunWorkspacePath: vi.fn(async () => filePath),
      resolveRunAttachmentFile: vi.fn(async () => ({
        path: filePath,
        name: 'pasted.txt',
        messageId: 'user'
      })),
      onAgentEvent: vi.fn((handler: typeof onEvent) => {
        onEvent = handler
        return vi.fn()
      }),
      onCollaborationObserverEvent: vi.fn((handler: typeof onObserver) => {
        onObserver = handler
        return vi.fn()
      }),
      onStarted: vi.fn((handler: typeof onRestart) => {
        onRestart = handler
        return vi.fn()
      })
    }
  }
  beforeEach(async () => {
    root = await mkdtemp(join(tmpdir(), 'read-tool-preview-'))
    await mkdir(join(root, 'workspace'))
    await mkdir(join(root, 'outside'))
    filePath = join(root, 'outside', 'notes.md')
    await writeFile(filePath, '# Outside workspace')
    await writeFile(join(root, 'workspace', 'inside.txt'), 'workspace')
    stored = {
      id: 'conversation',
      projectId: 'project',
      title: 'Preview',
      createdAt: 1,
      updatedAt: 1,
      messages: [{ id: 'assistant', role: 'assistant', content: '', createdAt: 1 }]
    }
    persistRead()
    source = createSource()
    resolver = new ReadToolFileSource(source)
    service = new WorkspaceFilesService(
      async () => ({
        id: 'project',
        folders: [
          {
            id: 'primary',
            alias: 'app',
            path: join(root, 'workspace'),
            role: 'primary',
            sortOrder: 0,
            createdAt: 1
          }
        ]
      }),
      undefined,
      undefined,
      resolver
    )
  })
  afterEach(async () => {
    resolver.dispose()
    await rm(root, { recursive: true, force: true })
  })

  it('previews an authorized outside file while directory browsing stays inside the workspace', async () => {
    const preview = await service.readPreview(request())
    expect(preview.text?.content).toBe('# Outside workspace')
    expect(source.resolveRunWorkspacePath).toHaveBeenCalledWith({
      projectId: 'project',
      assistantMessageId: 'assistant',
      filePath
    })
    expect(source.loadProjects).not.toHaveBeenCalled()
    expect(
      (await service.listDirectory({ projectId: 'project' })).entries.map((entry) => entry.path)
    ).toEqual(['inside.txt'])
    expect(await service.resolvePathForReveal(request())).toBe(await realpath(filePath))
  })

  it.each(['notes.md', '@workspace/frozen/notes.md', '@home/notes.md', 'C:\\reports\\notes.md'])(
    'passes the original %s source to the frozen resolver without current-root conversion',
    async (path) => {
      persistRead(path)
      expect((await service.readPreview(request({ filePath: path }))).text?.content).toBe(
        '# Outside workspace'
      )
      expect(source.resolveRunWorkspacePath).toHaveBeenCalledWith({
        projectId: 'project',
        assistantMessageId: 'assistant',
        filePath: path
      })
    }
  )

  it('supports projectless outside reads bound to the originating assistant', async () => {
    stored.projectId = null
    expect((await service.readPreview(request({ projectId: null }))).text?.content).toBe(
      '# Outside workspace'
    )
    expect(source.resolveRunWorkspacePath).toHaveBeenCalledWith({
      projectId: null,
      assistantMessageId: 'assistant',
      filePath
    })
  })

  it('rejects forged paths, calls, ownership and unsupported non-read calls', async () => {
    for (const change of [
      { filePath: join(root, 'workspace', 'inside.txt') },
      { callId: 'forged' },
      { assistantMessageId: 'other' },
      { projectId: 'other' },
      { conversationId: 'other' }
    ])
      await expect(service.readPreview(request(change))).rejects.toThrow('not available')
    persistRead(filePath, 'run_command')
    await expect(service.readPreview(request())).rejects.toThrow('not available')
    expect(source.resolveRunWorkspacePath).not.toHaveBeenCalled()
  })

  it('uses validated live observer proof before the read call is persisted', async () => {
    stored.messages[0].agentRunJson = '{}'
    const event: AgentEvent = {
      type: 'tool_call',
      runId: 'run',
      traceSequence: 1,
      identity: { type: 'builtin', toolName: 'read_file' },
      call: {
        id: 'read',
        tool: 'read_file',
        args: { path: filePath },
        approvalStatus: 'not_required',
        reason: null
      }
    }
    onObserver({
      schemaVersion: 1,
      rootAgentId: 'root',
      rootConversationId: 'conversation',
      agentId: 'root',
      conversationId: 'conversation',
      assistantMessageId: 'assistant',
      runId: 'run',
      event
    })
    expect((await service.readPreview(request())).text?.content).toBe('# Outside workspace')
    await expect(service.readPreview(request({ assistantMessageId: 'other' }))).rejects.toThrow(
      'not available'
    )
    onRestart()
    await expect(service.readPreview(request())).rejects.toThrow('not available')
  })

  it('rejects same-name live calls without authoritative builtin identity', async () => {
    stored.messages[0].agentRunJson = JSON.stringify({ runId: 'run', toolCalls: [] })
    onEvent({
      type: 'tool_call',
      runId: 'run',
      traceSequence: 1,
      identity: { type: 'runtime_extension', extensionId: 'external', toolName: 'read_file' },
      call: {
        id: 'read',
        tool: 'read_file',
        args: { path: filePath },
        approvalStatus: 'not_required',
        reason: null
      }
    })
    await expect(service.readPreview(request())).rejects.toThrow('not available')
    expect(source.resolveRunWorkspacePath).not.toHaveBeenCalled()
  })

  it('rejects explicit nonbuiltin persisted identities while retaining historical receipts without identity', async () => {
    for (const identity of [
      { type: 'runtime_extension', extensionId: 'external', toolName: 'read_file' },
      { type: 'builtin', toolName: 'different_tool' }
    ]) {
      persistRead()
      const run = JSON.parse(stored.messages[0].agentRunJson!)
      run.timeline = [{ type: 'tool_call', callId: 'read', identity }]
      stored.messages[0].agentRunJson = JSON.stringify(run)
      await expect(service.readPreview(request())).rejects.toThrow('not available')
    }
    expect(source.resolveRunWorkspacePath).not.toHaveBeenCalled()
    persistRead()
    expect((await service.readPreview(request())).text?.content).toBe('# Outside workspace')
  })

  it('uses live root call proof only when the stored assistant has that run identity', async () => {
    stored.messages[0].agentRunJson = JSON.stringify({ runId: 'run', toolCalls: [] })
    onEvent({
      type: 'tool_call',
      runId: 'run',
      traceSequence: 1,
      identity: { type: 'builtin', toolName: 'read_file' },
      call: {
        id: 'read',
        tool: 'read_file',
        args: { path: filePath },
        approvalStatus: 'not_required',
        reason: null
      }
    })
    expect((await service.readPreview(request())).text?.content).toBe('# Outside workspace')
  })

  it('keeps a running preview valid when the completed read reports another exact path spelling', async () => {
    const originalRequest = request()
    expect((await service.readPreview(originalRequest)).text?.content).toBe('# Outside workspace')
    const reportedPath = '@workspace/frozen/notes.md'
    onEvent({
      type: 'tool_result',
      runId: 'run',
      result: {
        callId: 'read',
        tool: 'read_file',
        ok: true,
        result: { path: reportedPath }
      }
    })
    expect((await service.readPreview(originalRequest)).text?.content).toBe('# Outside workspace')
    expect(await service.resolvePathForReveal(originalRequest)).toBe(await realpath(filePath))
    expect(source.resolveRunWorkspacePath).toHaveBeenLastCalledWith({
      projectId: 'project',
      assistantMessageId: 'assistant',
      filePath
    })
    expect((await service.readPreview(request({ filePath: reportedPath }))).text?.content).toBe(
      '# Outside workspace'
    )
    expect(source.resolveRunWorkspacePath).toHaveBeenLastCalledWith({
      projectId: 'project',
      assistantMessageId: 'assistant',
      filePath: reportedPath
    })
    await expect(service.readPreview(request({ filePath: 'notes.md' }))).rejects.toThrow(
      'not available'
    )
  })

  it('resolves pasted attachment identity rather than treating its virtual path as a local file', async () => {
    const path = '@attachments/pasted/pasted.txt'
    persistRead(path)
    stored.messages.push({
      id: 'user',
      role: 'user',
      content: '',
      createdAt: 0,
      attachments: [
        {
          id: 'pasted',
          kind: 'file',
          name: 'pasted.txt',
          mimeType: 'text/plain',
          sizeBytes: 19,
          createdAt: 0
        }
      ]
    })
    expect((await service.readPreview(request({ filePath: path }))).text?.content).toBe(
      '# Outside workspace'
    )
    expect(source.resolveRunAttachmentFile).toHaveBeenCalledWith({
      conversationId: 'conversation',
      assistantMessageId: 'assistant',
      filePath: path
    })
    expect(source.resolveRunWorkspacePath).not.toHaveBeenCalled()
  })

  it('resolves inherited project attachments through the originating turn without requiring a local listing', async () => {
    const path = '@attachments/project-file/pasted.txt'
    persistRead(path)
    expect((await service.readPreview(request({ filePath: path }))).text?.content).toBe(
      '# Outside workspace'
    )
    expect(source.resolveRunAttachmentFile).toHaveBeenCalledWith({
      conversationId: 'conversation',
      assistantMessageId: 'assistant',
      filePath: path
    })
    await expect(
      service.readPreview(request({ filePath: '@attachments/project-file/other.txt' }))
    ).rejects.toThrow('not available')
    expect(source.resolveRunAttachmentFile).toHaveBeenCalledTimes(1)
  })

  it('propagates Core rejection of attachment paths outside the originating turn scope', async () => {
    const path = '@attachments/unknown/pasted.txt'
    persistRead(path)
    source.resolveRunAttachmentFile.mockRejectedValueOnce(new Error('Attachment not available'))
    await expect(service.readPreview(request({ filePath: path }))).rejects.toThrow('not available')
    expect(source.resolveRunWorkspacePath).not.toHaveBeenCalled()
  })

  it('never falls back to current roots after frozen resolution fails', async () => {
    source.resolveRunWorkspacePath.mockRejectedValueOnce(new Error('Frozen root unavailable'))
    await expect(service.readPreview(request())).rejects.toThrow('Frozen root unavailable')
    expect(source.loadProjects).not.toHaveBeenCalled()
  })

  it('does not preview a directory or symlink returned by a source resolver', async () => {
    source.resolveRunWorkspacePath.mockResolvedValueOnce(root)
    await expect(service.readPreview(request())).rejects.toThrow('not available')
    const link = join(root, 'link')
    await symlink(filePath, link)
    source.resolveRunWorkspacePath.mockResolvedValueOnce(link)
    await expect(service.readPreview(request())).rejects.toThrow('not available')
  })
})
