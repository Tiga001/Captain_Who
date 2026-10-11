import { readFile, writeFile } from 'node:fs/promises'
import os from 'node:os'
import { performance } from 'node:perf_hooks'
import { createHash } from 'node:crypto'
import { createServer } from 'vite'

const out = new URL('./renderer-serialization-baseline.json', import.meta.url)
const fixture = JSON.parse(await readFile('packages/protocol/fixtures/persisted-agent-run-projection-v1.json', 'utf8'))
const server = await createServer({ configFile: false, server: { middlewareMode: true }, appType: 'custom' })
try {
  const { stringifyPersistedAgentRun, parsePersistedAgentRunJson } = await server.ssrLoadModule('/src/renderer/src/features/storage/persistedAgentRun.ts')
  const cases = []
  for (const [name, rows, width, samples] of [['short', 1, 128, 1000], ['medium', 100, 256, 300], ['long_trace', 1000, 512, 60]]) {
    const run = structuredClone(fixture.expectedCanonical)
    run.timeline = Array.from({ length: rows }, (_, i) => ({ id: `trace-message-${i}`, type: 'message', content: '确定性fake内容 '.repeat(Math.ceil(width / 13)).slice(0, width), traceSequence: i }))
    run.messageStreamCheckpoints = {}
    const canonical = stringifyPersistedAgentRun(run)
    if (parsePersistedAgentRunJson(canonical).timeline.length !== rows) throw new Error('projection fact count changed')
    const envelope = JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'storage.saveChatMessageState', params: { conversationId: 'fake-conversation', message: { id: 'fake-assistant', content: '正文'.repeat(500), status: 'sent', agentRunJson: canonical } } })
    for (let i = 0; i < 20; i++) stringifyPersistedAgentRun(run)
    const mapping = []
    const wire = []
    for (let i = 0; i < samples; i++) {
      let start = performance.now()
      const encoded = stringifyPersistedAgentRun(run)
      mapping.push(performance.now() - start)
      start = performance.now()
      JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'storage.saveChatMessageState', params: { conversationId: 'fake-conversation', message: { id: 'fake-assistant', content: '正文'.repeat(500), status: 'sent', agentRunJson: encoded } } })
      wire.push(performance.now() - start)
    }
    const summary = (values) => {
      values.sort((a,b)=>a-b)
      return { medianMs: values[Math.floor(values.length * .5)], p95Ms: values[Math.ceil(values.length * .95)-1], maxMs: values.at(-1), totalMs: values.reduce((a,b)=>a+b,0) }
    }
    cases.push({ name, rows, samples, projectionUtf8Bytes: Buffer.byteLength(canonical), illustrativeEnvelopeUtf8Bytes: Buffer.byteLength(envelope), sha256: createHash('sha256').update(canonical).digest('hex'), mapping: summary(mapping), illustrativeEnvelope: summary(wire) })
  }
  await writeFile(out, JSON.stringify({ at: new Date().toISOString(), node: process.version, platform: process.platform, arch: process.arch, cpu: os.cpus()[0]?.model, warmup: 20, caveats: ['Advisory Node/Vite SSR microbenchmark; not real browser IPC/paint latency.', 'Other build/test work may run concurrently; timing is not an isolated performance gate.', 'Envelope is illustrative JSON-RPC shape and excludes Host protocol validation; UTF-8 byte counts and fixture hashes are deterministic.', 'No actual Host, model, database or external messages were invoked.'], cases }, null, 2)+'\n')
  process.stdout.write(JSON.stringify(cases, null, 2)+'\n')
} finally { await server.close() }
