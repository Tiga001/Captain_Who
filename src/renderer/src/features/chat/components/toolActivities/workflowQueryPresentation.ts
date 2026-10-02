import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import type { SettledToolStatus } from './toolActivityUtils'

export interface WorkflowQueryProps {
  call: AgentToolCall
  result?: AgentToolResult
  cancelled?: boolean
  settledStatus?: SettledToolStatus
}
export type RecordValue = Record<string, unknown>
export type Localize = (zh: string, en: string) => string
export function record(value: unknown): RecordValue {
  return value && typeof value === 'object' && !Array.isArray(value) ? (value as RecordValue) : {}
}
export function records(value: unknown): RecordValue[] {
  return Array.isArray(value)
    ? value.filter((item) => item && typeof item === 'object' && !Array.isArray(item))
    : []
}
export function text(value: unknown): string {
  return typeof value === 'string' ? value : ''
}
export function queryStatus({ result, cancelled, settledStatus }: WorkflowQueryProps) {
  if (result?.ok === false) return 'failed'
  if (result) return record(result.result).available === true ? 'completed' : 'unavailable'
  if (cancelled || settledStatus === 'cancelled') return 'cancelled'
  if (settledStatus === 'failed') return 'failed'
  return settledStatus === 'completed' ? 'unknown' : 'running'
}
export function queryLabel(props: WorkflowQueryProps, subject: string, l: Localize): string {
  switch (queryStatus(props)) {
    case 'running':
      return l(`正在查看${subject}`, `Checking ${subject}`)
    case 'completed':
      return l(`已查看${subject}`, `Checked ${subject}`)
    case 'failed':
      return l(`查看${subject}失败`, `Could not check ${subject}`)
    case 'cancelled':
      return l(`已取消查看${subject}`, `Cancelled checking ${subject}`)
    case 'unavailable':
      return l('组织信息暂不可用', 'Organization information unavailable')
    default:
      return l(`${subject}的查询结果待确认`, `Query result for ${subject} is unconfirmed`)
  }
}
const states: Record<string, [string, string]> = {
  pending: ['待处理', 'Pending'],
  processing: ['处理中', 'Processing'],
  processed: ['已处理', 'Processed'],
  stopped: ['已停止', 'Stopped'],
  failed: ['失败', 'Failed'],
  recalled: ['已撤回', 'Recalled']
}
export function stateLabel(value: unknown, l: Localize): string {
  const pair = states[text(value)]
  return pair ? l(...pair) : l('状态未知', 'Status unknown')
}
export function stateTone(value: unknown): string {
  const state = text(value)
  return state === 'failed'
    ? 'danger'
    : state === 'stopped'
      ? 'attention'
      : state === 'processing' || state === 'processed'
        ? 'active'
        : 'muted'
}
export function timeLabel(value: unknown, language: string): string {
  if (typeof value !== 'number' || !Number.isFinite(value)) return ''
  const date = new Date(value)
  return Number.isNaN(date.valueOf())
    ? ''
    : date.toLocaleString(language, {
        month: 'short',
        day: 'numeric',
        hour: '2-digit',
        minute: '2-digit'
      })
}
