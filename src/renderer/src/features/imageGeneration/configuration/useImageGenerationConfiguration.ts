// Renderer image-generation configuration state machine: serializes CAS mutations and secrets.
import type {
  ImageGenerationCredentialMutation,
  ImageGenerationConfiguration,
  ImageGenerationConfigurationErrorCode,
  ImageGenerationUpdateConfigurationInput
} from '@mycopilot/protocol'
import { IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION } from '@mycopilot/protocol'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { hostClient } from '../../../host/hostClient'
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
  const backgroundRefreshPendingRef = useRef(false)
  const formBaselineRef = useRef<ImageGenerationConfiguration | undefined>(undefined)
  const draftRevisionRef = useRef(0)
  const cleanDraftRevisionRef = useRef(0)
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
      formBaselineRef.current = output.configuration
      cleanDraftRevisionRef.current = draftRevisionRef.current
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

  const refreshInBackground = useCallback(async () => {
    if (mutationInFlightRef.current) {
      backgroundRefreshPendingRef.current = true
      return
    }
    const sequence = ++requestSequenceRef.current
    try {
      const output = await getImageGenerationConfiguration()
      if (!mountedRef.current || sequence !== requestSequenceRef.current) return
      setConfiguration(output.configuration)
      // A capability change must not discard an unsaved endpoint, model or
      // credential. Keep the draft's CAS base too, rather than silently rebasing
      // an old configuration edit over another window's saved fields.
      if (draftRevisionRef.current === cleanDraftRevisionRef.current) {
        formBaselineRef.current = output.configuration
        setForm(formFromConfiguration(output.configuration))
        setCredentialMutation({ type: 'keep' })
      }
      setLoading(false)
      setLoadErrorCode(undefined)
    } catch (error) {
      // Retain the authoritative state and draft; reconnect/focus will retry.
      if (
        mountedRef.current &&
        sequence === requestSequenceRef.current &&
        !formBaselineRef.current
      ) {
        setLoading(false)
        setLoadErrorCode(getImageGenerationConfigurationErrorDetails(error).code)
      }
    }
  }, [])

  useEffect(() => {
    const changed = () => void refreshInBackground()
    const visible = () => {
      if (document.visibilityState === 'visible') changed()
    }
    const unsubscribe = hostClient.imageGeneration.onChanged(changed)
    window.addEventListener('focus', changed)
    document.addEventListener('visibilitychange', visible)
    return () => {
      unsubscribe()
      window.removeEventListener('focus', changed)
      document.removeEventListener('visibilitychange', visible)
    }
  }, [refreshInBackground])

  const dirty = useMemo(
    () => Boolean(configuration && form && isFormDirty(form, configuration, credentialMutation)),
    [configuration, credentialMutation, form]
  )

  const handleMutationError = useCallback(async (error: unknown) => {
    if (!mountedRef.current) return
    const details = getImageGenerationConfigurationErrorDetails(error)
    if (shouldRefreshImageGenerationConfiguration(details)) {
      const sequence = ++requestSequenceRef.current
      try {
        const { configuration: latest } = await getImageGenerationConfiguration()
        if (!mountedRef.current || sequence !== requestSequenceRef.current) return
        const baseline = formBaselineRef.current
        const latestForm = formFromConfiguration(latest)
        setConfiguration(latest)
        // Keep the user's edited fields/credential intention, but refresh fields
        // they did not edit. A subsequent explicit Save uses this new CAS base;
        // a conflict never silently discards the draft or retries the write.
        setForm((draft) =>
          !draft || !baseline
            ? latestForm
            : {
                endpointUrl:
                  draft.endpointUrl.trim() === baseline.endpointUrl
                    ? latestForm.endpointUrl
                    : draft.endpointUrl,
                modelId:
                  draft.modelId.trim() === baseline.modelId ? latestForm.modelId : draft.modelId,
                imageToImage:
                  draft.imageToImage === baseline.capabilities.imageToImage
                    ? latestForm.imageToImage
                    : draft.imageToImage,
                watermark:
                  draft.watermark === baseline.defaults.watermark
                    ? latestForm.watermark
                    : draft.watermark
              }
        )
        formBaselineRef.current = latest
        setFeedback({ kind: 'authoritativeRefresh', code: details.code })
      } catch {
        if (mountedRef.current && sequence === requestSequenceRef.current)
          setFeedback({ kind: 'error', code: 'unavailable' })
      }
      return
    }
    if (mountedRef.current) setFeedback({ kind: 'error', code: details.code })
  }, [])

  const runMutation = useCallback(
    async (task: () => Promise<void>) => {
      if (mutationInFlightRef.current) return false
      mutationInFlightRef.current = true
      requestSequenceRef.current += 1
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
        if (mountedRef.current && backgroundRefreshPendingRef.current) {
          backgroundRefreshPendingRef.current = false
          void refreshInBackground()
        }
      }
    },
    [handleMutationError, refreshInBackground]
  )

  const save = useCallback(async () => {
    if (!configuration || !form) return
    const snapshotConfiguration = formBaselineRef.current ?? configuration
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
      formBaselineRef.current = output.configuration
      cleanDraftRevisionRef.current = draftRevisionRef.current
    })
  }, [configuration, credentialMutation, form, runMutation])

  const updateForm = useCallback(
    <Key extends keyof ImageGenerationConfigurationForm>(
      key: Key,
      value: ImageGenerationConfigurationForm[Key]
    ) => {
      draftRevisionRef.current += 1
      setForm((current) => (current ? { ...current, [key]: value } : current))
      setFeedback(undefined)
    },
    []
  )

  const updateCredentialMutation = useCallback((mutation: ImageGenerationCredentialMutation) => {
    draftRevisionRef.current += 1
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
