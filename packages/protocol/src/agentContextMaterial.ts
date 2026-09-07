import type { ConversationContextImageRef, ConversationTurnTraceItem } from './agent'
import {
  expectArray,
  expectEnum,
  expectNonEmptyString,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  expectString,
  hasAsciiControlCharacter,
  invalidProtocolValue
} from './skills/validation'

const byteLength = (value: string): number => new TextEncoder().encode(value).length

function safeText(value: string, context: string): void {
  const lower = value.trimStart().toLowerCase()
  if (lower.startsWith('data:') && lower.includes(';base64,')) {
    throw invalidProtocolValue(context, 'binary material is not a context reference')
  }
}

function parseImage(value: unknown, context: string): ConversationContextImageRef {
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['attachmentId', 'mimeType', 'sha256'], context)
  const attachmentId = expectNonEmptyString(record.attachmentId, `${context}.attachmentId`)
  const mimeType = expectNonEmptyString(record.mimeType, `${context}.mimeType`)
  const sha256 = expectString(record.sha256, `${context}.sha256`)
  if (
    byteLength(attachmentId) > 1024 ||
    hasAsciiControlCharacter(attachmentId) ||
    !mimeType.startsWith('image/') ||
    mimeType.length <= 'image/'.length ||
    byteLength(mimeType) > 128 ||
    /\s/u.test(mimeType) ||
    hasAsciiControlCharacter(mimeType) ||
    !/^sha256:[0-9a-f]{64}$/.test(sha256)
  )
    throw invalidProtocolValue(context, 'invalid immutable image reference')
  safeText(attachmentId, context)
  return { attachmentId, mimeType, sha256 }
}

/** Internal Host-authored context fact. This parser never grants capabilities or emits UI events. */
export function parseConversationContextMaterial(
  value: unknown,
  context = 'conversation context material'
): Extract<ConversationTurnTraceItem, { type: 'context_material' }> {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['type', 'sequence', 'eventId', 'materialKind', 'content', 'images', 'createdAt'],
    context
  )
  const eventId = expectNonEmptyString(record.eventId, `${context}.eventId`)
  const content = expectString(record.content, `${context}.content`)
  const materialKind = expectEnum(
    record.materialKind,
    ['input_attachment', 'skill_instructions', 'run_world_state'] as const,
    `${context}.materialKind`
  )
  const rawImages =
    record.images === undefined ? [] : expectArray(record.images, `${context}.images`)
  if (rawImages.length > 256) throw invalidProtocolValue(context, 'too many image references')
  const images = rawImages.map((image, index) => parseImage(image, `${context}.images[${index}]`))
  if (
    byteLength(eventId) > 512 ||
    hasAsciiControlCharacter(eventId) ||
    byteLength(content) > 4 * 1024 * 1024 ||
    (content.trim().length === 0 && images.length === 0) ||
    (materialKind !== 'input_attachment' && images.length > 0) ||
    new Set(images.map((image) => image.attachmentId)).size !== images.length
  ) {
    throw invalidProtocolValue(context, 'invalid context material identity or payload')
  }
  safeText(eventId, context)
  safeText(content, context)
  return {
    type: expectEnum(record.type, ['context_material'] as const, `${context}.type`),
    sequence: expectSafeInteger(record.sequence, `${context}.sequence`, 0),
    eventId,
    materialKind,
    content,
    ...(images.length ? { images } : {}),
    createdAt: expectSafeInteger(record.createdAt, `${context}.createdAt`, 0)
  }
}
