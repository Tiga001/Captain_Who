import {
  expectArray,
  expectBoolean,
  expectEnum,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  expectSchemaVersion,
  expectString,
  invalidProtocolValue
} from './skills/validation'

export const HUMAN_INTERACTION_SCHEMA_VERSION = 1 as const
export const HUMAN_INTERACTION_MAX_INPUT_BYTES = 262_144
export const HUMAN_INTERACTION_MAX_TITLE_BYTES = 8_192
export const HUMAN_INTERACTION_MAX_OPTION_BYTES = 2_048
export const HUMAN_INTERACTION_MAX_ANSWER_BYTES = 32_768
export const HUMAN_INTERACTION_GET_SETTINGS_METHOD = 'humanInteraction.getSettings'
export const HUMAN_INTERACTION_UPDATE_SETTINGS_METHOD = 'humanInteraction.updateSettings'
export const HUMAN_INTERACTION_LIST_REQUESTS_METHOD = 'humanInteraction.listRequests'
export const HUMAN_INTERACTION_SUBMIT_METHOD = 'humanInteraction.submit'
export const HUMAN_INTERACTION_IGNORE_METHOD = 'humanInteraction.ignore'
export const HUMAN_INTERACTION_SETTINGS_CHANGED_METHOD = 'humanInteraction.settingsChanged'
export const HUMAN_INTERACTION_REQUEST_CHANGED_METHOD = 'humanInteraction.requestChanged'

export interface HumanInteractionSettings {
  enabled: boolean
  revision: number
  updatedAt: number
}
export interface HumanInteractionSettingsUpdate {
  enabled: boolean
  expectedRevision: number
}
export type HumanInteractionSettingsGetInput = Record<string, never>
export interface HumanInteractionToolInput {
  questions: HumanInteractionQuestionInput[]
}
export interface HumanInteractionQuestionInput {
  title: string
  options?: string[] | null
}
export type HumanInteractionMode = 'sync' | 'async'
export type HumanInteractionRequestStatus = 'open' | 'submitted' | 'ignored' | 'cancelled'
export type HumanInteractionResponseKind = 'submitted' | 'ignored'
export type HumanInteractionDeliveryStatus =
  'pending' | 'bound' | 'applied' | 'cancelled' | 'failed'
export interface HumanInteractionOption {
  id: string
  label: string
}
export interface HumanInteractionQuestion {
  id: string
  title: string
  options: HumanInteractionOption[] | null
}
export type HumanInteractionAnswer =
  | { kind: 'option'; questionId: string; optionId: string }
  | { kind: 'text'; questionId: string; text: string }
  | { kind: 'skipped'; questionId: string }
export interface HumanInteractionResponse {
  responseId: string
  requestId: string
  submissionId: string
  kind: HumanInteractionResponseKind
  answers: HumanInteractionAnswer[]
  createdAt: number
}
export interface HumanInteractionDelivery {
  responseId: string
  status: HumanInteractionDeliveryStatus
  revision: number
  targetRunId: string | null
  userMessageId: string | null
  errorCode: string | null
}
export interface HumanInteractionRequestSnapshot {
  schemaVersion: typeof HUMAN_INTERACTION_SCHEMA_VERSION
  requestId: string
  sequence: number
  conversationId: string
  runId: string
  assistantMessageId: string
  toolCallId: string
  mode: HumanInteractionMode
  status: HumanInteractionRequestStatus
  revision: number
  policyRevision: number
  questions: HumanInteractionQuestion[]
  response: HumanInteractionResponse | null
  delivery: HumanInteractionDelivery | null
  createdAt: number
  updatedAt: number
}
export interface HumanInteractionSubmitInput {
  conversationId: string
  requestId: string
  expectedRevision: number
  submissionId: string
  answers: HumanInteractionAnswer[]
}
export interface HumanInteractionIgnoreInput {
  conversationId: string
  requestId: string
  expectedRevision: number
  submissionId: string
}
export interface HumanInteractionListInput {
  conversationId: string
  cursor: string | null
  limit: number
}
export interface HumanInteractionListOutput {
  items: HumanInteractionRequestSnapshot[]
  nextCursor: string | null
}

const encoder = new TextEncoder()

function boundedText(value: unknown, context: string, maxBytes: number): string {
  const text = expectString(value, context)
  if (!text.trim() || text.includes('\0') || encoder.encode(text).length > maxBytes) {
    throw invalidProtocolValue(context, 'expected non-empty bounded text')
  }
  return text
}

function identifier(value: unknown, context: string): string {
  const id = boundedText(value, context, 256)
  const hasControl = Array.from(id).some((character) => {
    const code = character.codePointAt(0)!
    return code <= 0x1f || (code >= 0x7f && code <= 0x9f)
  })
  if (id.trim() !== id || hasControl) {
    throw invalidProtocolValue(
      context,
      'expected an opaque identifier without surrounding whitespace or controls'
    )
  }
  return id
}

function nullable<T>(
  value: unknown,
  parse: (value: unknown, context: string) => T,
  context: string
): T | null {
  return value === null ? null : parse(value, context)
}

function cursor(value: unknown, context: string): string {
  return boundedText(value, context, 2_048)
}

function unique(values: readonly string[], context: string): void {
  if (new Set(values).size !== values.length) {
    throw invalidProtocolValue(context, 'duplicate identities or labels')
  }
}

function boundedPayload(value: unknown, context: string): void {
  if (encoder.encode(JSON.stringify(value)).length > HUMAN_INTERACTION_MAX_INPUT_BYTES) {
    throw invalidProtocolValue(context, 'payload exceeds the byte limit')
  }
}

export function parseHumanInteractionSettingsGetInput(
  value: unknown
): HumanInteractionSettingsGetInput {
  const context = 'human interaction settings get input'
  expectOnlyKeys(expectRecord(value, context), [], context)
  return {}
}

export function parseHumanInteractionSettings(value: unknown): HumanInteractionSettings {
  const context = 'human interaction settings'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['enabled', 'revision', 'updatedAt'], context)
  return {
    enabled: expectBoolean(record.enabled, `${context}.enabled`),
    revision: expectSafeInteger(record.revision, `${context}.revision`, 0),
    updatedAt: expectSafeInteger(record.updatedAt, `${context}.updatedAt`, 0)
  }
}

export function parseHumanInteractionSettingsUpdate(
  value: unknown
): HumanInteractionSettingsUpdate {
  const context = 'human interaction settings update'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['enabled', 'expectedRevision'], context)
  return {
    enabled: expectBoolean(record.enabled, `${context}.enabled`),
    expectedRevision: expectSafeInteger(record.expectedRevision, `${context}.expectedRevision`, 0)
  }
}

export function parseHumanInteractionToolInput(value: unknown): HumanInteractionToolInput {
  const context = 'human interaction tool input'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['questions'], context)
  const questions = expectArray(record.questions, `${context}.questions`).map((value, index) => {
    const questionContext = `${context}.questions[${index}]`
    const question = expectRecord(value, questionContext)
    expectOnlyKeys(question, ['title', 'options'], questionContext)
    const title = boundedText(
      question.title,
      `${questionContext}.title`,
      HUMAN_INTERACTION_MAX_TITLE_BYTES
    )
    if (question.options === undefined) return { title }
    if (question.options === null) return { title, options: null }
    const options = expectArray(question.options, `${questionContext}.options`).map((option) =>
      boundedText(option, `${questionContext}.options`, HUMAN_INTERACTION_MAX_OPTION_BYTES)
    )
    if (options.length === 0) throw invalidProtocolValue(questionContext, 'empty options')
    unique(
      options.map((option) => option.trim()),
      questionContext
    )
    return { title, options }
  })
  if (questions.length === 0) throw invalidProtocolValue(context, 'empty questions')
  const parsed = { questions }
  boundedPayload(parsed, context)
  return parsed
}

export function parseHumanInteractionQuestion(
  value: unknown,
  context = 'human interaction question'
): HumanInteractionQuestion {
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['id', 'title', 'options'], context)
  const options = nullable(
    record.options,
    (value, context) => {
      const options = expectArray(value, context).map((value) => {
        const option = expectRecord(value, context)
        expectOnlyKeys(option, ['id', 'label'], context)
        return {
          id: identifier(option.id, `${context}.id`),
          label: boundedText(option.label, `${context}.label`, HUMAN_INTERACTION_MAX_OPTION_BYTES)
        }
      })
      if (!options.length) throw invalidProtocolValue(context, 'empty options')
      unique(
        options.map((option) => option.id),
        context
      )
      unique(
        options.map((option) => option.label.trim()),
        context
      )
      return options
    },
    `${context}.options`
  )
  return {
    id: identifier(record.id, `${context}.id`),
    title: boundedText(record.title, `${context}.title`, HUMAN_INTERACTION_MAX_TITLE_BYTES),
    options
  }
}

export function parseHumanInteractionAnswer(
  value: unknown,
  context = 'human interaction answer'
): HumanInteractionAnswer {
  const record = expectRecord(value, context)
  const kind = expectEnum(record.kind, ['option', 'text', 'skipped'] as const, `${context}.kind`)
  expectOnlyKeys(
    record,
    ['kind', 'questionId', ...(kind === 'option' ? ['optionId'] : kind === 'text' ? ['text'] : [])],
    context
  )
  const questionId = identifier(record.questionId, `${context}.questionId`)
  if (kind === 'option')
    return { kind, questionId, optionId: identifier(record.optionId, `${context}.optionId`) }
  if (kind === 'text')
    return {
      kind,
      questionId,
      text: boundedText(record.text, `${context}.text`, HUMAN_INTERACTION_MAX_ANSWER_BYTES)
    }
  return { kind, questionId }
}

function answers(value: unknown, context: string, allowEmpty = false): HumanInteractionAnswer[] {
  const parsed = expectArray(value, context).map((value) =>
    parseHumanInteractionAnswer(value, context)
  )
  if (!allowEmpty && !parsed.length) throw invalidProtocolValue(context, 'empty answers')
  unique(
    parsed.map((answer) => answer.questionId),
    context
  )
  boundedPayload(parsed, context)
  return parsed
}

export function parseHumanInteractionResponse(
  value: unknown,
  context = 'human interaction response'
): HumanInteractionResponse {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['responseId', 'requestId', 'submissionId', 'kind', 'answers', 'createdAt'],
    context
  )
  const kind = expectEnum(record.kind, ['submitted', 'ignored'] as const, `${context}.kind`)
  const parsedAnswers = answers(record.answers, `${context}.answers`, kind === 'ignored')
  if (kind === 'ignored' && parsedAnswers.length)
    throw invalidProtocolValue(context, 'ignored responses cannot contain answers')
  return {
    responseId: identifier(record.responseId, `${context}.responseId`),
    requestId: identifier(record.requestId, `${context}.requestId`),
    submissionId: identifier(record.submissionId, `${context}.submissionId`),
    kind,
    answers: parsedAnswers,
    createdAt: expectSafeInteger(record.createdAt, `${context}.createdAt`, 0)
  }
}

export function parseHumanInteractionDelivery(
  value: unknown,
  context = 'human interaction delivery'
): HumanInteractionDelivery {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['responseId', 'status', 'revision', 'targetRunId', 'userMessageId', 'errorCode'],
    context
  )
  return {
    responseId: identifier(record.responseId, `${context}.responseId`),
    status: expectEnum(
      record.status,
      ['pending', 'bound', 'applied', 'cancelled', 'failed'] as const,
      `${context}.status`
    ),
    revision: expectSafeInteger(record.revision, `${context}.revision`, 0),
    targetRunId: nullable(record.targetRunId, identifier, `${context}.targetRunId`),
    userMessageId: nullable(record.userMessageId, identifier, `${context}.userMessageId`),
    errorCode: nullable(record.errorCode, identifier, `${context}.errorCode`)
  }
}

export function parseHumanInteractionRequestSnapshot(
  value: unknown,
  context = 'human interaction request'
): HumanInteractionRequestSnapshot {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'requestId',
      'sequence',
      'conversationId',
      'runId',
      'assistantMessageId',
      'toolCallId',
      'mode',
      'status',
      'revision',
      'policyRevision',
      'questions',
      'response',
      'delivery',
      'createdAt',
      'updatedAt'
    ],
    context
  )
  expectSchemaVersion(record, HUMAN_INTERACTION_SCHEMA_VERSION, context)
  const questions = expectArray(record.questions, `${context}.questions`).map((value) =>
    parseHumanInteractionQuestion(value, `${context}.questions`)
  )
  if (!questions.length) throw invalidProtocolValue(context, 'empty questions')
  unique(
    questions.map((question) => question.id),
    context
  )
  const snapshot: HumanInteractionRequestSnapshot = {
    schemaVersion: HUMAN_INTERACTION_SCHEMA_VERSION,
    requestId: identifier(record.requestId, `${context}.requestId`),
    sequence: expectSafeInteger(record.sequence, `${context}.sequence`, 1),
    conversationId: identifier(record.conversationId, `${context}.conversationId`),
    runId: identifier(record.runId, `${context}.runId`),
    assistantMessageId: identifier(record.assistantMessageId, `${context}.assistantMessageId`),
    toolCallId: identifier(record.toolCallId, `${context}.toolCallId`),
    mode: expectEnum(record.mode, ['sync', 'async'] as const, `${context}.mode`),
    status: expectEnum(
      record.status,
      ['open', 'submitted', 'ignored', 'cancelled'] as const,
      `${context}.status`
    ),
    revision: expectSafeInteger(record.revision, `${context}.revision`, 0),
    policyRevision: expectSafeInteger(record.policyRevision, `${context}.policyRevision`, 0),
    questions,
    response: nullable(record.response, parseHumanInteractionResponse, `${context}.response`),
    delivery: nullable(record.delivery, parseHumanInteractionDelivery, `${context}.delivery`),
    createdAt: expectSafeInteger(record.createdAt, `${context}.createdAt`, 0),
    updatedAt: expectSafeInteger(record.updatedAt, `${context}.updatedAt`, 0)
  }
  if (
    snapshot.updatedAt < snapshot.createdAt ||
    (snapshot.response && snapshot.response.requestId !== snapshot.requestId) ||
    (snapshot.delivery && snapshot.delivery.responseId !== snapshot.response?.responseId)
  ) {
    throw invalidProtocolValue(context, 'inconsistent request ownership or timestamps')
  }
  if (snapshot.status === 'submitted' || snapshot.status === 'ignored') {
    if (snapshot.response?.kind !== snapshot.status)
      throw invalidProtocolValue(context, 'missing terminal response')
  } else if (snapshot.response || snapshot.delivery) {
    throw invalidProtocolValue(context, 'unanswered request cannot contain a response or delivery')
  }
  if (snapshot.status === 'ignored' && (snapshot.mode !== 'async' || snapshot.delivery)) {
    throw invalidProtocolValue(
      context,
      'only asynchronous requests can be ignored without delivery'
    )
  }
  if (snapshot.response?.kind === 'submitted') {
    if (!snapshot.delivery) throw invalidProtocolValue(context, 'missing response delivery')
    const byId = new Map(snapshot.response.answers.map((answer) => [answer.questionId, answer]))
    if (byId.size !== questions.length) throw invalidProtocolValue(context, 'incomplete response')
    for (const question of questions) {
      const answer = byId.get(question.id)
      if (
        !answer ||
        (answer.kind === 'option' &&
          !question.options?.some((option) => option.id === answer.optionId))
      ) {
        throw invalidProtocolValue(context, 'answer does not match an immutable question')
      }
    }
  }
  return snapshot
}

function parseResponseIdentity(
  record: Record<string, unknown>,
  context: string
): HumanInteractionIgnoreInput {
  return {
    conversationId: identifier(record.conversationId, `${context}.conversationId`),
    requestId: identifier(record.requestId, `${context}.requestId`),
    expectedRevision: expectSafeInteger(record.expectedRevision, `${context}.expectedRevision`, 0),
    submissionId: identifier(record.submissionId, `${context}.submissionId`)
  }
}

export function parseHumanInteractionSubmitInput(value: unknown): HumanInteractionSubmitInput {
  const context = 'human interaction submit input'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['conversationId', 'requestId', 'expectedRevision', 'submissionId', 'answers'],
    context
  )
  return {
    ...parseResponseIdentity(record, context),
    answers: answers(record.answers, `${context}.answers`)
  }
}

export function parseHumanInteractionIgnoreInput(value: unknown): HumanInteractionIgnoreInput {
  const context = 'human interaction ignore input'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['conversationId', 'requestId', 'expectedRevision', 'submissionId'],
    context
  )
  return parseResponseIdentity(record, context)
}

export function parseHumanInteractionListInput(value: unknown): HumanInteractionListInput {
  const context = 'human interaction list input'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['conversationId', 'cursor', 'limit'], context)
  const limit = expectSafeInteger(record.limit, `${context}.limit`, 1)
  if (limit > 100) throw invalidProtocolValue(context, 'limit exceeds 100')
  return {
    conversationId: identifier(record.conversationId, `${context}.conversationId`),
    cursor: nullable(record.cursor, cursor, `${context}.cursor`),
    limit
  }
}

export function parseHumanInteractionListOutput(value: unknown): HumanInteractionListOutput {
  const context = 'human interaction list output'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['items', 'nextCursor'], context)
  const items = expectArray(record.items, `${context}.items`).map((value) =>
    parseHumanInteractionRequestSnapshot(value)
  )
  if (items.length > 100) throw invalidProtocolValue(context, 'too many items')
  unique(
    items.map((item) => item.requestId),
    context
  )
  if (items.some((item, index) => index > 0 && items[index - 1].sequence <= item.sequence)) {
    throw invalidProtocolValue(context, 'expected unique batch sequences in descending order')
  }
  return { items, nextCursor: nullable(record.nextCursor, cursor, `${context}.nextCursor`) }
}
