// Renderer UI.
import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import type { ChatAgentRunView, ChatReadActivity, ChatReadActivityKind } from './chatTypes'

const READ_ACTIVITY_KINDS: Partial<Record<string, ChatReadActivityKind>> = {
  read_file: 'file',
  read_image: 'image',
  read_pdf: 'pdf',
  read_presentation: 'presentation',
  read_spreadsheet: 'spreadsheet',
  read_word: 'word'
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === 'object' && !Array.isArray(value))
}

function stringValue(value: unknown) {
  return typeof value === 'string' ? value.trim() : ''
}

export function isReadActivityTool(tool: string) {
  return Boolean(READ_ACTIVITY_KINDS[tool])
}

export function getReadActivityKindForTool(tool: string) {
  return READ_ACTIVITY_KINDS[tool] ?? 'file'
}

function pathFromArgs(args: unknown) {
  if (!isRecord(args)) return ''
  return stringValue(args.path) || stringValue(args.filePath)
}

function fileNameFromPath(path: string) {
  const normalized = path.replace(/\\/g, '/').replace(/\/$/, '')
  const fileName = normalized.split('/').filter(Boolean).pop()
  return fileName || normalized || '文件'
}

function extensionFromFileName(fileName: string) {
  const lower = fileName.trim().toLowerCase()
  if (!lower.includes('.')) return undefined
  return lower.split('.').pop() || undefined
}

function normalizeImageDataUrl(value: unknown) {
  const dataUrl = stringValue(value)
  return dataUrl.startsWith('data:image/') ? dataUrl : undefined
}

function thumbnailDataUrlFromImagePayload(payload: Record<string, unknown>) {
  const directThumbnail = normalizeImageDataUrl(payload.thumbnailDataUrl)
  if (directThumbnail) return directThumbnail

  const image = isRecord(payload.image) ? payload.image : undefined
  const nestedThumbnail = normalizeImageDataUrl(image?.thumbnailDataUrl)
  if (nestedThumbnail) return nestedThumbnail

  return undefined
}

function fullDataUrlFromImagePayload(payload: Record<string, unknown>) {
  const image = isRecord(payload.image) ? payload.image : undefined
  const mimeType = stringValue(image?.mimeType) || stringValue(payload.mimeType)
  const dataBase64 = stringValue(image?.dataBase64)
  if (!mimeType.startsWith('image/') || !dataBase64 || dataBase64 === '[redacted]') {
    return undefined
  }

  return `data:${mimeType};base64,${dataBase64}`
}

function activityFromResult(
  previous: ChatReadActivity | undefined,
  result: AgentToolResult
): ChatReadActivity {
  const payload = isRecord(result.result) ? result.result : {}
  const path = stringValue(payload.path) || previous?.path || ''
  const fileName = path ? fileNameFromPath(path) : previous?.fileName || '文件'
  const mimeType = stringValue(payload.mimeType) || previous?.mimeType
  const kind = previous?.kind ?? getReadActivityKindForTool(result.tool)
  const fullDataUrl =
    kind === 'image' ? (fullDataUrlFromImagePayload(payload) ?? previous?.fullDataUrl) : undefined
  const thumbnailDataUrl =
    kind === 'image'
      ? (thumbnailDataUrlFromImagePayload(payload) ?? fullDataUrl ?? previous?.thumbnailDataUrl)
      : undefined

  return {
    callId: result.callId,
    tool: result.tool,
    kind,
    status: result.ok ? 'completed' : 'failed',
    path,
    fileName,
    extension: extensionFromFileName(fileName),
    mimeType: mimeType || undefined,
    thumbnailDataUrl,
    fullDataUrl,
    error: result.ok ? undefined : result.error,
    updatedAt: Date.now()
  }
}

function upsertActivity(
  activities: ChatReadActivity[] | undefined,
  activity: ChatReadActivity
): ChatReadActivity[] {
  const currentActivities = activities ?? []
  if (!currentActivities.some((candidate) => candidate.callId === activity.callId)) {
    return [...currentActivities, activity]
  }

  return currentActivities.map((candidate) =>
    candidate.callId === activity.callId ? activity : candidate
  )
}

export function readActivityFromCall(call: AgentToolCall): ChatReadActivity | null {
  if (!isReadActivityTool(call.tool)) return null

  const path = pathFromArgs(call.args)
  const fileName = fileNameFromPath(path)

  return {
    callId: call.id,
    tool: call.tool,
    kind: getReadActivityKindForTool(call.tool),
    status: 'running',
    path,
    fileName,
    extension: extensionFromFileName(fileName),
    updatedAt: Date.now()
  }
}

export function upsertReadActivityFromCall(
  run: ChatAgentRunView,
  call: AgentToolCall
): ChatReadActivity[] | undefined {
  const activity = readActivityFromCall(call)
  if (!activity) return run.readActivities

  const existing = run.readActivities?.find((candidate) => candidate.callId === call.id)
  return upsertActivity(run.readActivities, {
    ...activity,
    fileName: existing?.fileName ?? activity.fileName,
    path: existing?.path ?? activity.path,
    extension: existing?.extension ?? activity.extension,
    mimeType: existing?.mimeType,
    thumbnailDataUrl: existing?.thumbnailDataUrl,
    fullDataUrl: existing?.fullDataUrl,
    error: existing?.error,
    status: existing?.status ?? activity.status
  })
}

export function upsertReadActivityFromResult(
  run: ChatAgentRunView,
  result: AgentToolResult
): ChatReadActivity[] | undefined {
  if (!isReadActivityTool(result.tool)) return run.readActivities

  const previous = run.readActivities?.find((activity) => activity.callId === result.callId)
  return upsertActivity(run.readActivities, activityFromResult(previous, result))
}

export function normalizeReadActivities(run: ChatAgentRunView): ChatReadActivity[] {
  const activities = run.readActivities ?? []

  return run.toolCalls.reduce<ChatReadActivity[]>((currentActivities, call) => {
    if (!isReadActivityTool(call.tool)) return currentActivities

    const callActivity =
      currentActivities.find((activity) => activity.callId === call.id) ??
      readActivityFromCall(call)
    if (!callActivity) return currentActivities

    const result = run.toolResults.find((candidate) => candidate.callId === call.id)
    const nextActivity = result ? activityFromResult(callActivity, result) : callActivity
    return upsertActivity(currentActivities, nextActivity)
  }, activities)
}

export function settlePendingReadActivities(
  run: ChatAgentRunView,
  status: ChatReadActivity['status'],
  settledAt: number
): ChatReadActivity[] | undefined {
  return run.toolCalls.reduce<ChatReadActivity[] | undefined>((activities, call) => {
    if (!isReadActivityTool(call.tool)) return activities
    if (run.toolResults.some((result) => result.callId === call.id)) return activities

    const previous = activities?.find((activity) => activity.callId === call.id)
    const activity = previous ?? readActivityFromCall(call)
    if (!activity || activity.status !== 'running') return activities

    return upsertActivity(activities, {
      ...activity,
      status,
      error: status === 'failed' ? (activity.error ?? run.error) : activity.error,
      updatedAt: settledAt
    })
  }, normalizeReadActivities(run))
}

export function cancelPendingReadActivities(
  run: ChatAgentRunView,
  cancelledAt: number
): ChatReadActivity[] | undefined {
  return settlePendingReadActivities(run, 'cancelled', cancelledAt)
}
