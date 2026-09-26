import type { AgentFolderReference, AgentInputAttachment } from '@mycopilot/protocol'
import type { StorageComposerDraftRecord } from '@mycopilot/protocol'
import type { ChatComposerDraft, ChatQueuedMessage, ChatWorkspaceMention } from '../chat/chatTypes'
import { normalizeSkillSelections } from '../skills/skillSelection'
import {
  normalizeStoredComposerPermissionMode,
  serializeComposerPermissionMode
} from './composerPermissionModePersistence'

export function mapDraftsFromStorage(
  drafts: StorageComposerDraftRecord[]
): Record<string, ChatComposerDraft> {
  return Object.fromEntries(drafts.map((draft) => [draft.scopeId, mapDraftFromStorage(draft)]))
}

export function mapDraftFromStorage(draft: StorageComposerDraftRecord): ChatComposerDraft {
  return {
    message: draft.message,
    permissionMode: normalizeStoredComposerPermissionMode(
      draft.permissionMode,
      draft.permissionModeVersion
    ),
    modelId: draft.modelId ?? '',
    projectId: draft.projectId ?? null,
    attachments: parseDraftAttachments(draft.attachmentsJson),
    folderReferences: parseFolderReferences(draft.folderReferencesJson),
    skills: parseDraftSkills(draft.skillsJson),
    queuedMessages: parseQueuedMessages(draft.queuedMessagesJson),
    updatedAt: draft.updatedAt
  }
}

export function mapDraftToStorage(
  scopeId: string,
  draft: ChatComposerDraft
): StorageComposerDraftRecord {
  return {
    scopeId,
    message: draft.message,
    ...serializeComposerPermissionMode(draft.permissionMode),
    modelId: draft.modelId || null,
    projectId: draft.projectId,
    attachmentsJson: JSON.stringify(draft.attachments),
    folderReferencesJson: JSON.stringify(draft.folderReferences ?? []),
    skillsJson: JSON.stringify(normalizeSkillSelections(draft.skills)),
    queuedMessagesJson: JSON.stringify(draft.queuedMessages),
    updatedAt: draft.updatedAt
  }
}

const COMPOSER_DRAFT_CORRUPTION_ERROR = 'Stored composer draft is malformed'

function isUnknownRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function parseDraftArray(value: string): unknown[] {
  try {
    const parsed = JSON.parse(value) as unknown
    if (Array.isArray(parsed)) return parsed
  } catch {
    // The stable error below deliberately excludes persisted content.
  }
  throw new Error(COMPOSER_DRAFT_CORRUPTION_ERROR)
}

function isExactRecord(
  value: unknown,
  required: readonly string[],
  optional: readonly string[] = []
): value is Record<string, unknown> {
  if (!isUnknownRecord(value)) return false
  const allowed = new Set([...required, ...optional])
  return (
    required.every((key) => Object.hasOwn(value, key)) &&
    Object.keys(value).every((key) => allowed.has(key))
  )
}

function parseDraftAttachments(value: string): AgentInputAttachment[] {
  return parseDraftArray(value).map((candidate) => {
    if (
      !isExactRecord(
        candidate,
        ['id', 'kind', 'name', 'sizeBytes', 'encoding', 'data'],
        ['mimeType', 'contentSha256', 'truncated']
      ) ||
      typeof candidate.id !== 'string' ||
      (candidate.kind !== 'file' && candidate.kind !== 'image') ||
      typeof candidate.name !== 'string' ||
      (candidate.mimeType !== undefined && typeof candidate.mimeType !== 'string') ||
      !Number.isSafeInteger(candidate.sizeBytes) ||
      (candidate.sizeBytes as number) < 0 ||
      candidate.encoding !== 'managed' ||
      typeof candidate.data !== 'string' ||
      (candidate.contentSha256 !== undefined &&
        (typeof candidate.contentSha256 !== 'string' ||
          !/^sha256:[0-9a-f]{64}$/u.test(candidate.contentSha256))) ||
      (candidate.truncated !== undefined && typeof candidate.truncated !== 'boolean')
    ) {
      throw new Error(COMPOSER_DRAFT_CORRUPTION_ERROR)
    }
    return candidate as unknown as AgentInputAttachment
  })
}

export function parseFolderReferences(value: string | null | undefined): AgentFolderReference[] {
  if (value == null || value === '') return []
  return parseDraftArray(value).map((candidate) => {
    const rootIdentity = normalizeStoredFolderRootIdentity(
      isUnknownRecord(candidate) ? candidate.rootIdentity : undefined
    )
    const normalizedCandidate = isUnknownRecord(candidate)
      ? { ...candidate, ...(rootIdentity === undefined ? {} : { rootIdentity }) }
      : candidate
    if (
      !isExactRecord(
        normalizedCandidate,
        ['schemaVersion', 'id', 'name'],
        ['rootPath', 'rootIdentity', 'status']
      ) ||
      normalizedCandidate.schemaVersion !== 1 ||
      typeof normalizedCandidate.id !== 'string' ||
      typeof normalizedCandidate.name !== 'string' ||
      (normalizedCandidate.rootPath !== undefined &&
        typeof normalizedCandidate.rootPath !== 'string') ||
      !isValidStoredFolderRootIdentity(rootIdentity) ||
      (normalizedCandidate.status !== undefined &&
        normalizedCandidate.status !== 'available' &&
        normalizedCandidate.status !== 'unavailable')
    ) {
      throw new Error(COMPOSER_DRAFT_CORRUPTION_ERROR)
    }
    return normalizedCandidate as unknown as AgentFolderReference
  })
}

function normalizeStoredFolderRootIdentity(value: unknown): unknown {
  if (!isUnknownRecord(value)) return value
  if (value.schemaVersion !== undefined) return value
  if (typeof value.schema_version !== 'number') return value
  if (value.kind === 'unix') {
    return {
      kind: value.kind,
      schemaVersion: value.schema_version,
      device: value.device,
      inode: value.inode
    }
  }
  if (value.kind === 'windows') {
    return {
      kind: value.kind,
      schemaVersion: value.schema_version,
      volumeSerialNumber: value.volume_serial_number,
      fileId: value.file_id
    }
  }
  return value
}

function isValidStoredFolderRootIdentity(value: unknown): boolean {
  return (
    value === undefined ||
    (isExactRecord(value, ['kind', 'schemaVersion', 'device', 'inode']) &&
      value.kind === 'unix' &&
      value.schemaVersion === 1 &&
      Number.isSafeInteger(value.device) &&
      Number.isSafeInteger(value.inode)) ||
    (isExactRecord(value, ['kind', 'schemaVersion', 'volumeSerialNumber', 'fileId']) &&
      value.kind === 'windows' &&
      value.schemaVersion === 1 &&
      Number.isSafeInteger(value.volumeSerialNumber) &&
      typeof value.fileId === 'string')
  )
}

function parseDraftSkills(value: string): ChatComposerDraft['skills'] {
  const parsed = parseDraftArray(value)
  const normalized = normalizeSkillSelections(parsed)
  if (
    normalized.length !== parsed.length ||
    parsed.some(
      (candidate, index) =>
        !isExactRecord(candidate, ['id', 'revision']) ||
        candidate.id !== normalized[index]?.id ||
        candidate.revision !== normalized[index]?.revision
    )
  ) {
    throw new Error(COMPOSER_DRAFT_CORRUPTION_ERROR)
  }
  return normalized
}

function parseQueuedMessages(value: string): ChatQueuedMessage[] {
  return parseDraftArray(value).map((candidate) => {
    if (
      !isExactRecord(
        candidate,
        [
          'id',
          'clientMessageId',
          'content',
          'attachments',
          'modelId',
          'permissionMode',
          'projectId',
          'skills',
          'status',
          'createdAt'
        ],
        ['error', 'folderReferences', 'workspaceMentions']
      ) ||
      typeof candidate.id !== 'string' ||
      typeof candidate.clientMessageId !== 'string' ||
      typeof candidate.content !== 'string' ||
      !Array.isArray(candidate.attachments) ||
      typeof candidate.modelId !== 'string' ||
      !['default', 'custom', 'full'].includes(String(candidate.permissionMode)) ||
      (candidate.projectId !== null && typeof candidate.projectId !== 'string') ||
      !Array.isArray(candidate.skills) ||
      !['pending', 'submitting', 'error'].includes(String(candidate.status)) ||
      !Number.isSafeInteger(candidate.createdAt) ||
      (candidate.createdAt as number) < 0 ||
      (candidate.error !== undefined && typeof candidate.error !== 'string')
    ) {
      throw new Error(COMPOSER_DRAFT_CORRUPTION_ERROR)
    }
    const attachments = parseDraftAttachments(JSON.stringify(candidate.attachments))
    const folderReferences = parseFolderReferences(JSON.stringify(candidate.folderReferences ?? []))
    const workspaceMentions = parseWorkspaceMentions(candidate.workspaceMentions)
    const skills = parseDraftSkills(JSON.stringify(candidate.skills))
    return {
      id: candidate.id,
      clientMessageId: candidate.clientMessageId,
      content: candidate.content,
      attachments,
      folderReferences,
      workspaceMentions,
      modelId: candidate.modelId,
      permissionMode: candidate.permissionMode as ChatQueuedMessage['permissionMode'],
      projectId: candidate.projectId,
      skills,
      status: candidate.status === 'error' ? 'error' : 'pending',
      ...(candidate.status === 'error' && candidate.error !== undefined
        ? { error: candidate.error }
        : {}),
      createdAt: candidate.createdAt as number
    }
  })
}

function parseWorkspaceMentions(value: unknown): ChatWorkspaceMention[] {
  if (value === undefined) return []
  if (!Array.isArray(value)) throw new Error(COMPOSER_DRAFT_CORRUPTION_ERROR)
  return value.map((candidate) => {
    if (
      !isExactRecord(candidate, [
        'id',
        'projectId',
        'folderId',
        'alias',
        'displayName',
        'path',
        'displayPath',
        'kind'
      ]) ||
      typeof candidate.id !== 'string' ||
      typeof candidate.projectId !== 'string' ||
      typeof candidate.folderId !== 'string' ||
      typeof candidate.alias !== 'string' ||
      typeof candidate.displayName !== 'string' ||
      typeof candidate.path !== 'string' ||
      typeof candidate.displayPath !== 'string' ||
      (candidate.kind !== 'file' && candidate.kind !== 'directory')
    ) {
      throw new Error(COMPOSER_DRAFT_CORRUPTION_ERROR)
    }
    return candidate as unknown as ChatWorkspaceMention
  })
}
