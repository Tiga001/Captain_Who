// Renderer image-generation projection: derives safe UI state from persisted ToolCall/ToolResult.
import type {
  AgentImageGenerationArtifact,
  AgentImageGenerationOperation,
  AgentImageGenerationResult,
  AgentToolCall,
  AgentToolResult
} from '@mycopilot/protocol'
import { parseAgentImageGenerationResult } from '@mycopilot/protocol'
import type { ChatAgentRunView } from '../chat/chatTypes'
import type { SettledToolStatus } from '../chat/components/toolActivities/toolActivityUtils'

export type ImageGenerationActivityOperation = AgentImageGenerationOperation | 'unknown'

export type ImageGenerationActivityStatus =
  'running' | 'completed' | 'failed' | 'cancelled' | 'outcomeIndeterminate' | 'commitIndeterminate'

export interface ImageGenerationActivityView {
  failureMessage?: string
  failureRecovery?: string
  operation: ImageGenerationActivityOperation
  reason?: string
  status: ImageGenerationActivityStatus
}

export interface ImageGenerationArtifactEntry {
  artifact: AgentImageGenerationArtifact
  callId: string
  operation: AgentImageGenerationOperation
}

export function getImageGenerationActivityView(
  call: AgentToolCall,
  result?: AgentToolResult,
  settledStatus?: SettledToolStatus
): ImageGenerationActivityView {
  const parsedResult = parseResult(result)
  const callOperation = getCallOperation(call)
  const operation =
    callOperation === 'unknown' ? (parsedResult?.operation ?? 'unknown') : callOperation
  const reason = getCallReason(call)

  if (!result) {
    return {
      operation,
      ...(reason ? { reason } : {}),
      status:
        settledStatus === 'cancelled'
          ? 'cancelled'
          : settledStatus === 'failed'
            ? 'failed'
            : settledStatus === 'completed'
              ? 'outcomeIndeterminate'
              : 'running'
    }
  }

  if (!parsedResult || (parsedResult.status === 'succeeded' && !result.ok)) {
    return { operation, ...(reason ? { reason } : {}), status: 'failed' }
  }

  if (parsedResult.status === 'succeeded') {
    return { operation, ...(reason ? { reason } : {}), status: 'completed' }
  }

  return {
    operation,
    ...(reason ? { reason } : {}),
    failureMessage: parsedResult.failure.message,
    failureRecovery: parsedResult.failure.recovery,
    status: parsedResult.status
  }
}

/**
 * Successful cards are derived exclusively from strict v1 Tool results. Markdown links, private
 * paths, and provider URLs never participate in Artifact identity or history restoration.
 */
export function getImageGenerationArtifactEntries(
  run: ChatAgentRunView
): ImageGenerationArtifactEntry[] {
  const entries: ImageGenerationArtifactEntry[] = []
  const seenArtifactIds = new Set<string>()

  for (const call of run.toolCalls) {
    const result = run.toolResults.find((candidate) => candidate.callId === call.id)
    const entry = getImageGenerationArtifactEntry(call, result)
    if (!entry || seenArtifactIds.has(entry.artifact.artifactId)) continue

    seenArtifactIds.add(entry.artifact.artifactId)
    entries.push(entry)
  }

  return entries
}

export function getImageGenerationArtifactEntry(
  call: AgentToolCall,
  result?: AgentToolResult
): ImageGenerationArtifactEntry | undefined {
  if (call.tool !== 'image_generation' || !result?.ok) return undefined
  const parsed = parseResult(result)
  if (!parsed || parsed.status !== 'succeeded') return undefined
  return { artifact: parsed.artifact, callId: call.id, operation: parsed.operation }
}

function parseResult(result?: AgentToolResult): AgentImageGenerationResult | undefined {
  if (!result || result.result === undefined) return undefined
  try {
    return parseAgentImageGenerationResult(result.result)
  } catch {
    // Preflight failures and future protocol versions intentionally degrade to a safe generic UI.
    return undefined
  }
}

function getCallOperation(call: AgentToolCall): ImageGenerationActivityOperation {
  if (!isRecord(call.args)) return 'unknown'
  const request = call.args.request
  if (!isRecord(request)) return 'unknown'
  return request.operation === 'generate' || request.operation === 'edit'
    ? request.operation
    : 'unknown'
}

function getCallReason(call: AgentToolCall): string | undefined {
  if (typeof call.reason === 'string' && call.reason.trim()) return call.reason.trim()
  if (!isRecord(call.args)) return undefined
  return typeof call.args.reason === 'string' && call.args.reason.trim()
    ? call.args.reason.trim()
    : undefined
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === 'object' && !Array.isArray(value))
}
