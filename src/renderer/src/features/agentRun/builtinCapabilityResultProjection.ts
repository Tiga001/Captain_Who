import {
  parseBrowserArtifactToolProjection,
  type AgentToolResult,
  type BrowserArtifactReference,
  type BrowserDownloadReference
} from '@mycopilot/protocol'

const SAFE_BUILTIN_TOOL_STATUSES = new Set([
  'completed',
  'failed',
  'cancelled',
  'outcome_unknown',
  'rejected',
  'expired',
  'payload_unavailable'
])

/**
 * Renderer-safe projection for every built-in capability Tool result channel.
 *
 * Most results already arrive through the Core persistence/event projection. A rejected
 * sensitive Tool is the exception: its action-decision response intentionally contains
 * process-only user feedback for the model. This boundary keeps only the typed terminal status
 * and reviewed Artifact references, so feedback, raw page data, errors, and future unknown fields
 * cannot enter chat state or Renderer persistence.
 */
export function projectBuiltinCapabilityToolResult(result: AgentToolResult): AgentToolResult {
  const record = asRecord(result.result)
  const status = safeBuiltinToolStatus(record)
  let artifacts: BrowserArtifactReference[] | undefined
  let downloads: BrowserDownloadReference[] | undefined
  try {
    const projection = parseBrowserArtifactToolProjection(result.result)
    artifacts = projection.artifacts
    downloads = projection.downloads
  } catch {
    artifacts = undefined
    downloads = undefined
  }

  return {
    callId: result.callId,
    tool: result.tool,
    ok: result.ok,
    ...(status
      ? {
          result: {
            schemaVersion: 1,
            type: 'builtin_capability_tool',
            status,
            contentOmitted: true,
            ...(artifacts ? { artifacts } : {}),
            ...(downloads ? { downloads } : {})
          }
        }
      : {})
  }
}

function safeBuiltinToolStatus(record: Record<string, unknown> | null): string | undefined {
  if (
    !record ||
    record.schemaVersion !== 1 ||
    record.contentOmitted !== true ||
    (record.type !== 'builtin_capability_tool' && record.type !== 'builtin_mcp_tool_approval') ||
    typeof record.status !== 'string' ||
    !SAFE_BUILTIN_TOOL_STATUSES.has(record.status)
  ) {
    return undefined
  }
  if (record.type === 'builtin_mcp_tool_approval' && record.status !== 'rejected') {
    return undefined
  }
  return record.status
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null
}
