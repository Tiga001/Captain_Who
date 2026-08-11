import type { AgentCommandPublishedOutput } from '@mycopilot/protocol'

const MAX_MANAGED_COMMAND_OUTPUTS = 32
const SHA256_PATTERN = /^[0-9a-f]{64}$/u

function hasAsciiControlCharacter(value: string): boolean {
  for (const character of value) {
    const codePoint = character.codePointAt(0)
    if (codePoint !== undefined && (codePoint <= 0x1f || codePoint === 0x7f)) return true
  }
  return false
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

function positiveSafeInteger(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isSafeInteger(value) && value > 0 ? value : undefined
}

function parseOutput(value: unknown): AgentCommandPublishedOutput | undefined {
  if (!isRecord(value)) return undefined
  const name = typeof value.name === 'string' ? value.name.trim() : ''
  const kind = value.kind === 'image' || value.kind === 'document' ? value.kind : undefined
  const readPath = typeof value.readPath === 'string' ? value.readPath : ''
  const mimeType = typeof value.mimeType === 'string' ? value.mimeType : ''
  const sizeBytes = positiveSafeInteger(value.sizeBytes)
  const sha256 = typeof value.sha256 === 'string' ? value.sha256 : ''
  if (
    !name ||
    name.length > 1024 ||
    hasAsciiControlCharacter(name) ||
    !kind ||
    !sizeBytes ||
    !SHA256_PATTERN.test(sha256)
  ) {
    return undefined
  }

  const expectedReadPath =
    kind === 'image' ? `image-artifact://sha256/${sha256}` : `artifact://sha256/${sha256}`
  if (readPath !== expectedReadPath) return undefined

  if (kind === 'document') {
    if (mimeType !== 'application/pdf') return undefined
    return { name, kind, readPath, mimeType, sizeBytes, sha256 }
  }

  const width = positiveSafeInteger(value.width)
  const height = positiveSafeInteger(value.height)
  if (!['image/png', 'image/jpeg', 'image/webp'].includes(mimeType) || !width || !height) {
    return undefined
  }
  return { name, kind, readPath, mimeType, sizeBytes, sha256, width, height }
}

/** Strictly projects the bounded, presentation-safe receipts emitted by managed commands. */
export function parseManagedCommandOutputs(
  value: unknown
): AgentCommandPublishedOutput[] | undefined {
  if (value === undefined) return undefined
  if (!Array.isArray(value) || value.length > MAX_MANAGED_COMMAND_OUTPUTS) return undefined
  const outputs = value.map(parseOutput)
  return outputs.every((output): output is AgentCommandPublishedOutput => output !== undefined)
    ? outputs
    : undefined
}

export function managedCommandOutputsEqual(
  left: AgentCommandPublishedOutput[] | undefined,
  right: AgentCommandPublishedOutput[] | undefined
): boolean {
  if (left === right) return true
  if (!left || !right || left.length !== right.length) return false
  return left.every((output, index) => {
    const candidate = right[index]
    return (
      candidate !== undefined &&
      output.name === candidate.name &&
      output.kind === candidate.kind &&
      output.readPath === candidate.readPath &&
      output.mimeType === candidate.mimeType &&
      output.sizeBytes === candidate.sizeBytes &&
      output.sha256 === candidate.sha256 &&
      output.width === candidate.width &&
      output.height === candidate.height
    )
  })
}
