import { useLayoutEffect, useRef } from 'react'
import { useAccountAuth } from '../auth/AccountAuthContext'
import { useLicense } from './LicenseContext'

/** In-flight user intent belongs to one continuous login, not whichever account is next. */
export function useTurnAccessIdentity() {
  const auth = useAccountAuth()
  const license = useLicense()
  const account = `${auth?.state.status ?? 'absent'}:${auth?.state.profile?.userId ?? ''}`
  const identity = `${account}:${license?.state.status ?? 'absent'}`
  const ref = useRef({ identity, account, generation: 0, accountGeneration: 0 })
  useLayoutEffect(() => {
    if (ref.current.identity !== identity) {
      ref.current = {
        identity,
        account,
        generation: ref.current.generation + 1,
        accountGeneration: ref.current.accountGeneration + (ref.current.account === account ? 0 : 1)
      }
    }
  }, [identity, account])
  return ref
}
