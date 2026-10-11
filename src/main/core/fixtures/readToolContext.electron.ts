import assert from 'node:assert/strict'
import { mkdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { app } from 'electron'
import type { StorageChatMessageRecord } from '@mycopilot/protocol'
import { CoreJsonRpcClient } from '../jsonRpcClient'
import { CoreServerStorageApi } from '../coreServerStorageApi'
import { RpcDiagnostics } from '../rpcDiagnostics'

const [profile, binary] = process.argv.slice(2)
if (!profile || !binary) throw new Error('Isolated profile and prebuilt Core binary are required')
mkdirSync(profile, { recursive: true })
app.setPath('userData', profile)
app.setPath('sessionData', join(profile, 'session'))

async function main(): Promise<void> {
  await app.whenReady()
  const rpc = new CoreJsonRpcClient({ appDataRoot: join(profile, 'core-data') })
  // Test-only executable injection. Production command/environment selection is unchanged, and
  // the real client still owns stdin/stdout, serialization, response matching and shutdown.
  Reflect.set(rpc, 'resolveCommand', () => ({ executable: binary, args: [], cwd: dirname(binary) }))
  const observations: object[] = []
  const diagnostics = new RpcDiagnostics(true, (summary) => observations.push(summary))
  Reflect.set(rpc, 'diagnostics', diagnostics)
  const api = new (class extends CoreServerStorageApi {
    constructor() {
      super(rpc)
    }
  })()
  try {
    await api.saveConversationMeta({
      id: 'synthetic',
      title: 'Synthetic read cost',
      createdAt: 1,
      updatedAt: 1
    })
    const messages: StorageChatMessageRecord[] = Array.from({ length: 256 }, (_, index) => ({
      id: `message-${index}`,
      role: index % 2 ? 'assistant' : 'user',
      content: 'synthetic retained context '.repeat(160),
      createdAt: index + 1,
      status: 'sent',
      ...(index % 2
        ? {
            agentRunJson: JSON.stringify({
              runId: `run-${index}`,
              toolCalls: [{ id: 'read', tool: 'read_file', args: { path: 'notes.txt' } }],
              toolResults: [
                {
                  callId: 'read',
                  tool: 'read_file',
                  result: { path: 'notes.txt', content: 'synthetic result '.repeat(256) }
                }
              ]
            })
          }
        : {})
    }))
    await api.upsertChatMessages({ conversationId: 'synthetic', messages, positionOffset: 0 })
    const input = { conversationId: 'synthetic', assistantMessageId: 'message-255', callId: 'read' }
    const full = await api.loadConversation('synthetic')
    assert(full)
    const projected = JSON.parse(
      full.messages.find((message) => message.id === input.assistantMessageId)!.agentRunJson!
    )
    const context = await api.loadReadToolFileContext(input)
    assert(context)
    assert.equal(context.call?.path, projected.toolCalls[0].args.path)
    assert.equal(context.resultPath, projected.toolResults[0].result.path)
    assert.equal(context.runId, projected.runId)
    assert.equal(context.projectId, full.projectId ?? null)
    assert.equal(context.messageFound, true)
    diagnostics.flush()
    observations.length = 0
    const iterations = 20
    const fullCpuStart = process.cpuUsage()
    for (let index = 0; index < iterations; index++) await api.loadConversation('synthetic')
    const fullHostCpuMicros = process.cpuUsage(fullCpuStart)
    diagnostics.flush()
    const fullMetrics = observations.splice(0)
    const narrowCpuStart = process.cpuUsage()
    for (let index = 0; index < iterations; index++) await api.loadReadToolFileContext(input)
    const narrowHostCpuMicros = process.cpuUsage(narrowCpuStart)
    diagnostics.flush()
    const narrowMetrics = observations.splice(0)
    await api.deleteConversation('synthetic')
    assert.equal(await api.loadReadToolFileContext(input), null)
    rpc.beginShutdown()
    await rpc.requestDuringShutdown('core.shutdown')
    console.log(
      'MYCOPILOT_READ_TOOL_CONTEXT=' +
        JSON.stringify({
          iterations,
          messages: messages.length,
          proofEqual: true,
          deletionObserved: true,
          fullMetrics,
          narrowMetrics,
          fullHostCpuMicros,
          narrowHostCpuMicros,
          scope:
            'Real Electron Host/Core pipe; sequential synthetic reads, no model, no file access, no UI paint or saturation claim'
        })
    )
  } finally {
    rpc.stop()
  }
}

void main().then(
  () => app.exit(0),
  (error: unknown) => {
    console.error(error)
    app.exit(1)
  }
)
