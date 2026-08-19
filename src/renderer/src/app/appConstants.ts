export const LEFT_DEFAULT_WIDTH = 288
export const RIGHT_DEFAULT_WIDTH = 360
export const LEFT_MIN_WIDTH = 220
export const RIGHT_MIN_WIDTH = 280
export const LEFT_MAX_WIDTH = 420
export const RIGHT_MAX_WIDTH = 1200
export const RIGHT_MAX_VIEWPORT_RATIO = 0.72
export const CENTER_MIN_WIDTH = 480
export const NEW_CONVERSATION_DRAFT_ID = 'new-conversation'

export function clamp(value: number, min: number, max: number) {
  return Math.min(Math.max(value, min), max)
}
