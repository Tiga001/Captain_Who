// Renderer image-generation configuration state machine: serializes CAS mutations and secrets.
import type {
  ImageGenerationCredentialMutation,
  ImageGenerationConfiguration,
  ImageGenerationConfigurationErrorCode,
  ImageGenerationUpdateConfigurationInput
} from '@mycopilot/protocol'
import { IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION } from '@mycopilot/protocol'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  getImageGenerationConfiguration,
  updateImageGenerationConfiguration
} from './imageGenerationClient'
import {
  getImageGenerationConfigurationErrorDetails,
  shouldRefreshImageGenerationConfiguration
} from './imageGenerationErrors'

export interface ImageGenerationConfigurationForm {
  endpointUrl: string
  imageToImage: boolean
  modelId: string
  watermark: boolean
}

export type ImageGenerationConfigurationPendingOperation = 'saving'

export type ImageGenerationConfigurationFeedback =
  | { kind: 'authoritativeRefresh'; code?: ImageGenerationConfigurationErrorCode }
  | { kind: 'error'; code?: ImageGenerationConfigurationErrorCode }

export type ImageGenerationConfigurationState =
  | { status: 'loading' }
  | { status: 'error'; code?: ImageGenerationConfigurationErrorCode }
  | {
      status: 'ready'
      configuration: ImageGenerationConfiguration
      form: ImageGenerationConfigurationForm
      credentialMutation: ImageGenerationCredentialMutation
      dirty: boolean
      feedback?: ImageGenerationConfigurationFeedback
      pendingOperation?: ImageGenerationConfigurationPendingOperation
    }

function formFromConfiguration(
  configuration: ImageGenerationConfiguration
): ImageGenerationConfigurationForm {
  return {
    endpointUrl: configuration.endpointUrl,
    imageToImage: configuration.capabilities.imageToImage,
    modelId: configuration.modelId,
    watermark: configuration.defaults.watermark
  }
}

function isFormDirty(
  form: ImageGenerationConfigurationForm,
  configuration: ImageGenerationConfiguration,
  credentialMutation: ImageGenerationCredentialMutation
): boolean {
  return (
    form.endpointUrl.trim() !== configuration.endpointUrl ||
    form.modelId.trim() !== configuration.modelId ||
    form.imageToImage !== configuration.capabilities.imageToImage ||
    form.watermark !== configuration.defaults.watermark ||
    credentialMutation.type !== 'keep'
  )
}

function updateInput(
  configuration: ImageGenerationConfiguration,
  form: ImageGenerationConfigurationForm,
  credentialMutation: ImageGenerationCredentialMutation
): ImageGenerationUpdateConfigurationInput {
  return {
    schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
    expectedRevision: configuration.revision,
    adapterId: 'smartmlSeedream',
    endpointUrl: form.endpointUrl.trim(),
    modelId: form.modelId.trim(),
    capabilities: {
      textToImage: true,
      imageToImage: form.imageToImage
    },
    defaults: {
      sizePreset: '2K',
      watermark: form.watermark
    },
    credentialMutation
  }
}

export function useImageGenerationConfiguration() {
  const mountedRef = useRef(false)
  const requestSequenceRef = useRef(0)
  const mutationInFlightRef = useRef(false)
  const [configuration, setConfiguration] = useState<ImageGenerationConfiguration>()
  const [form, setForm] = useState<ImageGenerationConfigurationForm>()
  const [credentialMutation, setCredentialMutation] = useState<ImageGenerationCredentialMutation>({
    type: 'keep'
  })
  const [loadErrorCode, setLoadErrorCode] = useState<ImageGenerationConfigurationErrorCode>()
  const [isLoading, setLoading] = useState(true)
  const [pendingOperation, setPendingOperation] =
    useState<ImageGenerationConfigurationPendingOperation>()
  const [feedback, setFeedback] = useState<ImageGenerationConfigurationFeedback>()

  const load = useCallback(async (options: { preserveFeedback?: boolean } = {}) => {
    const sequence = requestSequenceRef.current + 1
    requestSequenceRef.current = sequence
    setLoading(true)
    setLoadErrorCode(undefined)
    if (!options.preserveFeedback) setFeedback(undefined)

    try {
      const output = await getImageGenerationConfiguration()
      if (!mountedRef.current || requestSequenceRef.current !== sequence) return undefined
      setConfiguration(output.configuration)
      setForm(formFromConfiguration(output.configuration))
      setCredentialMutation({ type: 'keep' })
      return output.configuration
    } catch (error) {
      if (!mountedRef.current || requestSequenceRef.current !== sequence) return undefined
      const details = getImageGenerationConfigurationErrorDetails(error)
      setLoadErrorCode(details.code)
      return undefined
    } finally {
      if (mountedRef.current && requestSequenceRef.current === sequence) setLoading(false)
    }
  }, [])

  useEffect(() => {
    mountedRef.current = true
    void load()
    return () => {
      mountedRef.current = false
      requestSequenceRef.current += 1
    }
  }, [load])

  const dirty = useMemo(
    () => Boolean(configuration && form && isFormDirty(form, configuration, credentialMutation)),
    [configuration, credentialMutation, form]
  )

  const handleMutationError = useCallback(
    async (error: unknown) => {
      if (!mountedRef.current) return
      const details = getImageGenerationConfigurationErrorDetails(error)
      if (shouldRefreshImageGenerationConfiguration(details)) {
        const authoritativeConfiguration = await load()
        if (!mountedRef.current) return
        setFeedback(
          authoritativeConfiguration
            ? { kind: 'authoritativeRefresh', code: details.code }
            : { kind: 'error', code: 'unavailable' }
        )
        return
      }
      if (mountedRef.current) setFeedback({ kind: 'error', code: details.code })
    },
    [load]
  )

  const runMutation = useCallback(
    async (task: () => Promise<void>) => {
      if (mutationInFlightRef.current) return false
      mutationInFlightRef.current = true
      setPendingOperation('saving')
      setFeedback(undefined)
      try {
        await task()
        return true
      } catch (error) {
        await handleMutationError(error)
        return false
      } finally {
        mutationInFlightRef.current = false
        if (mountedRef.current) setPendingOperation(undefined)
      }
    },
    [handleMutationError]
  )

  const save = useCallback(async () => {
    if (!configuration || !form) return
    const snapshotConfiguration = configuration
    const snapshotForm = form
    const snapshotCredentialMutation = credentialMutation

    await runMutation(async () => {
      const output = await updateImageGenerationConfiguration(
        updateInput(snapshotConfiguration, snapshotForm, snapshotCredentialMutation)
      )
      if (!mountedRef.current) return
      setConfiguration(output.configuration)
      setForm(formFromConfiguration(output.configuration))
      setCredentialMutation({ type: 'keep' })
    })
  }, [configuration, credentialMutation, form, runMutation])

  const updateForm = useCallback(
    <Key extends keyof ImageGenerationConfigurationForm>(
      key: Key,
      value: ImageGenerationConfigurationForm[Key]
    ) => {
      setForm((current) => (current ? { ...current, [key]: value } : current))
      setFeedback(undefined)
    },
    []
  )

  const updateCredentialMutation = useCallback((mutation: ImageGenerationCredentialMutation) => {
    setCredentialMutation(mutation)
    setFeedback(undefined)
  }, [])

  const state: ImageGenerationConfigurationState =
    isLoading && !configuration
      ? { status: 'loading' }
      : !configuration || !form
        ? { status: 'error', code: loadErrorCode }
        : {
            status: 'ready',
            configuration,
            form,
            credentialMutation,
            dirty,
            ...(feedback ? { feedback } : {}),
            ...(pendingOperation ? { pendingOperation } : {})
          }

  return {
    load,
    save,
    updateCredentialMutation,
    state,
    updateForm
  }
}
