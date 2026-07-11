export const LEFT_DEFAULT_WIDTH = 288
export const RIGHT_DEFAULT_WIDTH = 360
export const LEFT_MIN_WIDTH = 220
export const RIGHT_MIN_WIDTH = 280
export const SIDE_MAX_WIDTH = 560
export const CENTER_MIN_WIDTH = 480
export const NEW_CONVERSATION_DRAFT_ID = 'new-conversation'

export const THINKING_PLACEHOLDER = '正在思考...'

export function clamp(value: number, min: number, max: number) {
  return Math.min(Math.max(value, min), max)
}
