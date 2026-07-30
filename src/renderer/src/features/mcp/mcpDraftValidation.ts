import { MCP_MANAGEMENT_LIMITS } from '@mycopilot/protocol'
import type { TranslationKey } from '../../config/frontendTranslations'
import type { McpServerDraft } from './mcpManagementInputs'

type Translate = (key: TranslationKey) => string

export function validateMcpDraft(draft: McpServerDraft, t: Translate): readonly string[] {
  const encoder = new TextEncoder()
  const errors: string[] = []
  if (!draft.displayName.trim()) errors.push(t('mcp.form.errorNameRequired'))
  if (encoder.encode(draft.displayName).byteLength > MCP_MANAGEMENT_LIMITS.displayNameBytes) {
    errors.push(t('mcp.form.errorNameTooLong'))
  }
  validatePath(draft.executable, 'mcp.form.errorExecutableRequired', errors, t, encoder)
  validatePath(draft.cwd, 'mcp.form.errorCwdRequired', errors, t, encoder)
  if (draft.arguments.length > MCP_MANAGEMENT_LIMITS.arguments) {
    errors.push(t('mcp.form.errorTooManyArguments'))
  }
  let totalBytes = 0
  for (const argument of draft.arguments) {
    if (argument.includes('\0')) errors.push(t('mcp.form.errorNul'))
    const bytes = encoder.encode(argument).byteLength
    totalBytes += bytes
    if (bytes > MCP_MANAGEMENT_LIMITS.argumentBytes) {
      errors.push(t('mcp.form.errorArgumentTooLong'))
      break
    }
  }
  if (totalBytes > MCP_MANAGEMENT_LIMITS.argumentsTotalBytes) {
    errors.push(t('mcp.form.errorArgumentsTooLarge'))
  }
  return [...new Set(errors)]
}

function validatePath(
  value: string,
  requiredKey: 'mcp.form.errorExecutableRequired' | 'mcp.form.errorCwdRequired',
  errors: string[],
  t: Translate,
  encoder: TextEncoder
): void {
  if (!value) errors.push(t(requiredKey))
  if (value.includes('\0')) errors.push(t('mcp.form.errorNul'))
  if (encoder.encode(value).byteLength > MCP_MANAGEMENT_LIMITS.pathBytes) {
    errors.push(t('mcp.form.errorPathTooLong'))
  }
}
