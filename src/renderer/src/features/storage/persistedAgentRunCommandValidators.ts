import { parseAgentCommandArtifactObservation } from '@mycopilot/protocol'
import type { ChatAgentRunView, ChatCommandSessionView } from '../chat/chatTypes'
import { parseManagedCommandOutputs } from '../chat/managedCommandOutputs'
import {
  MAX_STORED_RUN_ITEMS,
  hasExactKeys,
  hasOwn,
  isOptionalSafeInteger,
  isRecord,
  isSafeInteger
} from './persistedAgentRunValidation'

const TERMINAL_COMMAND_STATUSES = new Set<ChatCommandSessionView['status']>([
  'exited',
  'interrupted',
  'timed_out',
  'failed',
  'outcome_unknown'
])

export function parseCommandSessions(
  value: unknown,
  allowedCallIds: ReadonlySet<string>
): Record<string, ChatCommandSessionView> | undefined | null {
  if (value === undefined) return undefined
  if (!isRecord(value) || Object.keys(value).length > MAX_STORED_RUN_ITEMS) return null
  const sessions: Record<string, ChatCommandSessionView> = {}
  for (const [callId, raw] of Object.entries(value)) {
    if (!allowedCallIds.has(callId) || !isRecord(raw)) return null
    if (
      !hasExactKeys(
        raw,
        ['callId', 'status', 'latestSequence', 'outputTruncated'],
        ['startedAt', 'endedAt', 'exitCode', 'outputs', 'artifactObservation']
      ) ||
      raw.callId !== callId ||
      typeof raw.status !== 'string' ||
      !TERMINAL_COMMAND_STATUSES.has(raw.status as ChatCommandSessionView['status']) ||
      !isSafeInteger(raw.latestSequence) ||
      typeof raw.outputTruncated !== 'boolean' ||
      !isOptionalSafeInteger(raw, 'startedAt') ||
      !isOptionalSafeInteger(raw, 'endedAt') ||
      (hasOwn(raw, 'exitCode') && !isSafeInteger(raw.exitCode, Number.MIN_SAFE_INTEGER))
    ) {
      return null
    }
    if (hasOwn(raw, 'outputs')) {
      if (!Array.isArray(raw.outputs)) return null
      const outputKeys = ['name', 'kind', 'readPath', 'mimeType', 'sizeBytes', 'sha256'] as const
      const optionalOutputKeys = ['width', 'height'] as const
      if (
        !raw.outputs.every(
          (output) => isRecord(output) && hasExactKeys(output, outputKeys, optionalOutputKeys)
        )
      ) {
        return null
      }
    }
    const outputs = parseManagedCommandOutputs(raw.outputs)
    if (hasOwn(raw, 'outputs') && outputs === undefined) return null
    let artifactObservation: ChatCommandSessionView['artifactObservation']
    if (hasOwn(raw, 'artifactObservation')) {
      try {
        artifactObservation = parseAgentCommandArtifactObservation(raw.artifactObservation)
      } catch {
        return null
      }
    }
    sessions[callId] = {
      callId,
      status: raw.status as ChatCommandSessionView['status'],
      ...(typeof raw.startedAt === 'number' ? { startedAt: raw.startedAt } : {}),
      ...(typeof raw.endedAt === 'number' ? { endedAt: raw.endedAt } : {}),
      ...(typeof raw.exitCode === 'number' ? { exitCode: raw.exitCode } : {}),
      latestSequence: raw.latestSequence,
      outputTruncated: raw.outputTruncated,
      ...(outputs && outputs.length > 0 ? { outputs } : {}),
      ...(artifactObservation === undefined ? {} : { artifactObservation })
    }
  }
  return sessions
}

export function projectDurableCommandSessions(
  value: ChatAgentRunView['commandSessions'],
  allowedCallIds: ReadonlySet<string>
): Record<string, ChatCommandSessionView> | undefined {
  if (!value) return undefined
  const sessions = Object.fromEntries(
    Object.entries(value).flatMap(([callId, session]) => {
      if (!allowedCallIds.has(callId) || !TERMINAL_COMMAND_STATUSES.has(session.status)) return []
      const outputs = parseManagedCommandOutputs(session.outputs)
      return [
        [
          callId,
          {
            callId,
            status: session.status,
            ...(session.startedAt === undefined ? {} : { startedAt: session.startedAt }),
            ...(session.endedAt === undefined ? {} : { endedAt: session.endedAt }),
            ...(session.exitCode === undefined ? {} : { exitCode: session.exitCode }),
            latestSequence: session.latestSequence,
            outputTruncated: session.outputTruncated,
            ...(outputs && outputs.length > 0 ? { outputs } : {}),
            ...(session.artifactObservation === undefined
              ? {}
              : { artifactObservation: session.artifactObservation })
          }
        ]
      ]
    })
  ) as Record<string, ChatCommandSessionView>
  return Object.keys(sessions).length > 0 ? sessions : undefined
}
