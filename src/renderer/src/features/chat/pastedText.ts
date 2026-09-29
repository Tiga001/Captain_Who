import type { AgentInputAttachment } from '@mycopilot/protocol'

export const PASTED_TEXT_ATTACHMENT_THRESHOLD = 5_000

export function pastedTextMetadata(text: string): NonNullable<AgentInputAttachment['pastedText']> {
  return {
    preview: Array.from(text.replace(/\s+/gu, ' ').trim()).slice(0, 80).join(''),
    characterCount: text.length
  }
}

export interface EditorSelection {
  start: number
  end: number
}

/** Follow edits made while the full managed text is loading, without replacing newer input. */
export function restorePastedText(
  current: string,
  text: string,
  selection: EditorSelection,
  previous = current
): { message: string; cursor: number } {
  let { start, end } = selection
  if (previous !== current) {
    let prefix = 0
    while (
      prefix < previous.length &&
      prefix < current.length &&
      previous[prefix] === current[prefix]
    )
      prefix++
    let suffix = 0
    while (
      suffix < previous.length - prefix &&
      suffix < current.length - prefix &&
      previous[previous.length - suffix - 1] === current[current.length - suffix - 1]
    )
      suffix++
    const oldEnd = previous.length - suffix
    const newEnd = current.length - suffix
    const adjust = (position: number) =>
      position <= prefix ? position : position >= oldEnd ? position + newEnd - oldEnd : newEnd
    if (start < oldEnd && end > prefix) {
      // A newer edit inside the selected range belongs to the user. Insert after it instead
      // of letting an older asynchronous restore replace that input.
      start = newEnd
      end = newEnd
    } else {
      start = adjust(start)
      end = adjust(end)
    }
  }
  start = Math.max(0, Math.min(start, current.length))
  end = Math.max(start, Math.min(end, current.length))
  return {
    message: current.slice(0, start) + text + current.slice(end),
    cursor: start + text.length
  }
}
