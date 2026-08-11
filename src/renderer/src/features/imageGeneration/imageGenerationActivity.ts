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
import { parseManagedCommandOutputs } from '../chat/managedCommandOutputs'
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
  displayName?: string
  operation: AgentImageGenerationOperation | 'command'
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
 * Successful cards come only from strict generated-image results or authoritative managed-command
 * receipts. Markdown links, private paths, and provider URLs never participate in Artifact identity.
 */
export function getImageGenerationArtifactEntries(
  run: ChatAgentRunView
): ImageGenerationArtifactEntry[] {
  const entries: ImageGenerationArtifactEntry[] = []
  const seenArtifactIds = new Set<string>()

  for (const call of run.toolCalls) {
    const result = run.toolResults.find((candidate) => candidate.callId === call.id)
    const callEntries = [
      getImageGenerationArtifactEntry(call, result),
      ...getManagedCommandImageArtifactEntries(call, result),
      ...(call.tool === 'run_command'
        ? getManagedCommandImageArtifactEntriesFromOutputs(
            call.id,
            run.commandSessions?.[call.id]?.outputs
          )
        : [])
    ].filter((entry): entry is ImageGenerationArtifactEntry => entry !== undefined)
    for (const entry of callEntries) {
      if (seenArtifactIds.has(entry.artifact.artifactId)) continue
      seenArtifactIds.add(entry.artifact.artifactId)
      entries.push(entry)
    }
  }

  return entries
}

function getManagedCommandImageArtifactEntries(
  call: AgentToolCall,
  result?: AgentToolResult
): ImageGenerationArtifactEntry[] {
  if ((call.tool !== 'run_command' && call.tool !== 'command_session') || !result?.ok) return []
  const payload = isRecord(result.result) ? result.result : undefined
  const execution = isRecord(payload?.execution) ? payload.execution : undefined
  const outputs = Array.isArray(execution?.outputs)
    ? execution.outputs
    : Array.isArray(payload?.outputs)
      ? payload.outputs
      : []
  return getManagedCommandImageArtifactEntriesFromOutputs(call.id, outputs)
}

function getManagedCommandImageArtifactEntriesFromOutputs(
  callId: string,
  value: unknown
): ImageGenerationArtifactEntry[] {
  const outputs = parseManagedCommandOutputs(value) ?? []
  return outputs.flatMap((output) => {
    if (output.kind !== 'image') return []
    const { sha256, readPath, mimeType, name, sizeBytes, width, height } = output
    const format =
      mimeType === 'image/png'
        ? 'png'
        : mimeType === 'image/jpeg'
          ? 'jpeg'
          : mimeType === 'image/webp'
            ? 'webp'
            : undefined
    if (!format || !width || !height) {
      return []
    }
    return [
      {
        artifact: {
          artifactId: `sha256:${sha256}`,
          uri: readPath,
          kind: 'image',
          format,
          mimeType,
          width,
          height,
          sizeBytes,
          sha256
        },
        callId,
        displayName: name,
        operation: 'command' as const
      }
    ]
  })
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
