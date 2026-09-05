import {
  parseHumanInteractionSubmitInput,
  type HumanInteractionAnswer,
  type HumanInteractionRequestSnapshot,
  type HumanInteractionResponseDisplay
} from '@mycopilot/protocol'

export interface HumanInteractionDraft {
  readonly pageIndex: number
  readonly answers: Readonly<Record<string, HumanInteractionAnswer | undefined>>
}
export const EMPTY_HUMAN_INTERACTION_DRAFT: HumanInteractionDraft = { pageIndex: 0, answers: {} }

/** Request settlement and answer delivery have independent monotonic revisions. */
export function mergeHumanInteractionRequest(
  previous: HumanInteractionRequestSnapshot | undefined,
  incoming: HumanInteractionRequestSnapshot
): HumanInteractionRequestSnapshot {
  if (!previous) return incoming
  if (
    previous.conversationId !== incoming.conversationId ||
    previous.sequence !== incoming.sequence ||
    previous.runId !== incoming.runId ||
    previous.assistantMessageId !== incoming.assistantMessageId ||
    previous.toolCallId !== incoming.toolCallId ||
    previous.mode !== incoming.mode ||
    JSON.stringify(previous.questions) !== JSON.stringify(incoming.questions)
  )
    return previous
  // A terminal request is an enduring tombstone, including against malformed higher revisions.
  if (previous.status !== 'open' && incoming.status !== previous.status) return previous
  if (previous.response && incoming.response?.responseId !== previous.response.responseId)
    return previous
  let next = incoming.revision >= previous.revision ? incoming : previous
  if (previous.response) next = { ...next, response: previous.response }
  const oldDelivery = previous.delivery
  const newDelivery = incoming.delivery
  if (oldDelivery && newDelivery?.responseId === oldDelivery.responseId) {
    const terminal = ['applied', 'cancelled', 'failed'].includes(oldDelivery.status)
    const delivery =
      newDelivery.revision > oldDelivery.revision &&
      (!terminal || newDelivery.status === oldDelivery.status)
        ? newDelivery
        : oldDelivery
    next = { ...next, delivery }
  } else if (oldDelivery) next = { ...next, delivery: oldDelivery }
  return JSON.stringify(previous) === JSON.stringify(next) ? previous : next
}

export function humanInteractionAnswers(
  request: HumanInteractionRequestSnapshot,
  draft: HumanInteractionDraft
): HumanInteractionAnswer[] | null {
  try {
    const answers = request.questions.map((question) => {
      const answer = draft.answers[question.id]
      if (!answer || answer.questionId !== question.id) throw new Error('Incomplete answer')
      if (
        answer.kind === 'option' &&
        !question.options?.some((option) => option.id === answer.optionId)
      )
        throw new Error('Unknown option')
      return answer
    })
    return parseHumanInteractionSubmitInput({
      conversationId: request.conversationId,
      requestId: request.requestId,
      expectedRevision: request.revision,
      submissionId: '00000000-0000-4000-8000-000000000000',
      answers
    }).answers
  } catch {
    return null
  }
}

export function humanInteractionResponseDisplay(
  request: HumanInteractionRequestSnapshot
): HumanInteractionResponseDisplay | null {
  if (request.status !== 'submitted' || request.response?.kind !== 'submitted') return null
  const response = request.response
  return {
    type: 'human_interaction_response',
    schemaVersion: 1,
    requestId: request.requestId,
    responseId: response.responseId,
    answers: request.questions.map((question) => {
      const answer = response.answers.find((answer) => answer.questionId === question.id)!
      if (answer.kind === 'option')
        return {
          kind: 'option',
          questionId: question.id,
          question: question.title,
          optionId: answer.optionId,
          answer: question.options!.find((option) => option.id === answer.optionId)!.label
        }
      if (answer.kind === 'text')
        return {
          kind: 'text',
          questionId: question.id,
          question: question.title,
          answer: answer.text
        }
      return {
        kind: 'skipped',
        questionId: question.id,
        question: question.title,
        answer: '已跳过'
      }
    })
  }
}
