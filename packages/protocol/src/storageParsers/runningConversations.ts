import type { StorageRunningConversationSummary } from '../storage'
import {
  expectArray,
  expectNonEmptyString,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  expectString
} from '../skills/validation'

export function parseStorageRunningConversationSummaries(
  value: unknown
): StorageRunningConversationSummary[] {
  return expectArray(value, 'running conversation summaries').map((item, index) => {
    const context = `running conversation summaries[${index}]`
    const record = expectRecord(item, context)
    expectOnlyKeys(record, ['id', 'title', 'updatedAt'], context)
    return {
      id: expectNonEmptyString(record.id, `${context}.id`),
      title: expectString(record.title, `${context}.title`),
      updatedAt: expectSafeInteger(record.updatedAt, `${context}.updatedAt`, 0)
    }
  })
}
