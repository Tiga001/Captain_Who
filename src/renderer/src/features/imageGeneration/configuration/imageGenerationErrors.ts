// Renderer image-generation errors: classifies structured Host failures without matching text.
import { HostInvocationError } from '@mycopilot/host-api'
import type {
  ImageGenerationConfigurationErrorCode,
  ImageGenerationConfigurationRecovery
} from '@mycopilot/protocol'

export interface ImageGenerationConfigurationErrorDetails {
  code?: ImageGenerationConfigurationErrorCode
  configurationMayHaveChanged: boolean
  recovery?: ImageGenerationConfigurationRecovery
}

const ERROR_CODES = new Set<ImageGenerationConfigurationErrorCode>([
  'invalidRequest',
  'revisionConflict',
  'unsupportedAdapter',
  'missingEndpoint',
  'invalidEndpoint',
  'insecureEndpoint',
  'missingModel',
  'invalidModelId',
  'missingCredential',
  'invalidCredential',
  'credentialReplacementRequired',
  'storageUnavailable',
  'credentialStoreUnavailable',
  'commitIndeterminate',
  'unavailable'
])

const RECOVERIES = new Set<ImageGenerationConfigurationRecovery>([
  'fixConfiguration',
  'refreshConfiguration',
  'reenterCredential',
  'retry',
  'contactSupport'
])

export function getImageGenerationConfigurationErrorDetails(
  error: unknown
): ImageGenerationConfigurationErrorDetails {
  if (!(error instanceof HostInvocationError) || !isRecord(error.data)) {
    return { configurationMayHaveChanged: false }
  }

  const data = error.data
  if (data.type !== 'imageGenerationConfiguration') {
    return { configurationMayHaveChanged: false }
  }

  const code =
    typeof data.code === 'string' &&
    ERROR_CODES.has(data.code as ImageGenerationConfigurationErrorCode)
      ? (data.code as ImageGenerationConfigurationErrorCode)
      : undefined
  const recovery =
    typeof data.recovery === 'string' &&
    RECOVERIES.has(data.recovery as ImageGenerationConfigurationRecovery)
      ? (data.recovery as ImageGenerationConfigurationRecovery)
      : undefined

  return {
    code,
    configurationMayHaveChanged:
      data.configurationMayHaveChanged === true || code === 'commitIndeterminate',
    recovery
  }
}

export function shouldRefreshImageGenerationConfiguration(
  details: ImageGenerationConfigurationErrorDetails
): boolean {
  return (
    details.code === 'revisionConflict' ||
    details.configurationMayHaveChanged ||
    details.recovery === 'refreshConfiguration'
  )
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === 'object' && !Array.isArray(value))
}
