import { expectString, invalidProtocolValue, expectSafeInteger } from '../skills/validation'

export const ID_MAX = 256

export const TITLE_MAX = 512

export const PROMPT_MAX = 65_536

export const SAFE_TEXT_MAX = 4_096

export const CURSOR_MAX = 2_048

export const RRULE_MAX = 4_096

export const ZONE_MAX = 255

export const LIST_MAX = 100

export function boundedString(
  value: unknown,
  context: string,
  max: number,
  allowEmpty = false,
  allowFormattingControls = false
): string {
  const result = expectString(value, context)
  const byteLength = new TextEncoder().encode(result).byteLength
  const invalidControls = [...result].some((character) => {
    const code = character.charCodeAt(0)
    const isControl = code <= 31 || (code >= 127 && code <= 159)
    return isControl && (!allowFormattingControls || (character !== '\n' && character !== '\t'))
  })
  if ((!allowEmpty && result.trim().length === 0) || byteLength > max || invalidControls) {
    throw invalidProtocolValue(
      context,
      `must contain ${allowEmpty ? 'at most' : '1 to'} ${max} UTF-8 bytes without invalid controls`
    )
  }
  return result
}

export function nullableString(
  value: unknown,
  context: string,
  max: number,
  allowFormattingControls = false
): string | null {
  return value === null ? null : boundedString(value, context, max, true, allowFormattingControls)
}

export function nullableNonEmptyString(
  value: unknown,
  context: string,
  max: number
): string | null {
  return value === null ? null : boundedString(value, context, max)
}

export function nullableInteger(value: unknown, context: string): number | null {
  return value === null ? null : expectSafeInteger(value, context, 0)
}

export function intInRange(value: unknown, context: string, min: number, max: number): number {
  const result = expectSafeInteger(value, context, min)
  if (result > max) throw invalidProtocolValue(context, `must not exceed ${max}`)
  return result
}
