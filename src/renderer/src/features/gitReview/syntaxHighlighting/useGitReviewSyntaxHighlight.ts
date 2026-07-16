import { useEffect, useRef, useState } from 'react'
import {
  getGitReviewSyntaxHighlightManager,
  type GitReviewSyntaxHighlightManager
} from './GitReviewSyntaxHighlightManager'
import {
  createPlainGitReviewHighlightResult,
  type GitReviewHighlightInput,
  type GitReviewHighlightResult
} from './gitReviewSyntaxHighlightTypes'

export interface GitReviewSyntaxHighlightHookInput extends GitReviewHighlightInput {
  enabled?: boolean
}

export type GitReviewSyntaxHighlightState =
  { status: 'idle' } | { status: 'loading' } | { result: GitReviewHighlightResult; status: 'ready' }

interface InputSignature {
  cacheKey: string
  code: string
  language: string
}

interface InternalState {
  publicState: GitReviewSyntaxHighlightState
  signature?: InputSignature
}

const IDLE_STATE: GitReviewSyntaxHighlightState = { status: 'idle' }
const LOADING_STATE: GitReviewSyntaxHighlightState = { status: 'loading' }

/**
 * React lifecycle adapter for the shared service. Input changes synchronously hide stale tokens;
 * effect cleanup only cancels this consumer, leaving deduplicated requests used by other cards alive.
 */
export function useGitReviewSyntaxHighlight(
  input: GitReviewSyntaxHighlightHookInput,
  manager: GitReviewSyntaxHighlightManager = getGitReviewSyntaxHighlightManager()
): GitReviewSyntaxHighlightState {
  const enabled = input.enabled ?? true
  const versionRef = useRef(0)
  const [state, setState] = useState<InternalState>({ publicState: IDLE_STATE })

  useEffect(() => {
    versionRef.current += 1
    const version = versionRef.current
    if (!enabled) {
      setState({ publicState: IDLE_STATE })
      return
    }

    const signature: InputSignature = {
      cacheKey: input.cacheKey,
      code: input.code,
      language: input.language
    }
    const controller = new AbortController()
    setState({ publicState: LOADING_STATE, signature })

    void manager
      .highlight(signature, { signal: controller.signal })
      .then((result) => {
        if (controller.signal.aborted || versionRef.current !== version) return
        setState({ publicState: { result, status: 'ready' }, signature })
      })
      .catch((error: unknown) => {
        if (controller.signal.aborted || versionRef.current !== version || isAbortError(error)) {
          return
        }
        setState({
          publicState: {
            result: createPlainGitReviewHighlightResult(signature, 'worker-error'),
            status: 'ready'
          },
          signature
        })
      })

    return () => controller.abort()
  }, [enabled, input.cacheKey, input.code, input.language, manager])

  if (!enabled) return IDLE_STATE
  if (!sameSignature(state.signature, input)) return LOADING_STATE
  return state.publicState
}

function sameSignature(
  signature: InputSignature | undefined,
  input: GitReviewSyntaxHighlightHookInput
): boolean {
  if (!signature) return false
  return (
    signature.cacheKey === input.cacheKey &&
    signature.code === input.code &&
    signature.language === input.language
  )
}

function isAbortError(error: unknown): boolean {
  return error instanceof Error && error.name === 'AbortError'
}
