const REMOVED_CODE_POINTS = new Set([
  0x061c, 0x200b, 0x200c, 0x200d, 0x200e, 0x200f, 0x202a, 0x202b, 0x202c, 0x202d, 0x202e, 0x2060,
  0x2066, 0x2067, 0x2068, 0x2069, 0xfeff
])

export function toSafeMcpDisplayText(value: string, maximumCodePoints = 4096): string {
  let output = ''
  let written = 0

  for (const character of value) {
    const codePoint = character.codePointAt(0)
    if (
      codePoint === undefined ||
      codePoint <= 0x1f ||
      (codePoint >= 0x7f && codePoint <= 0x9f) ||
      REMOVED_CODE_POINTS.has(codePoint)
    ) {
      continue
    }
    if (written >= maximumCodePoints) break
    output += character
    written += 1
  }

  return output
}
