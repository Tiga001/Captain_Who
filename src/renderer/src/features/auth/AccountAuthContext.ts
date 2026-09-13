import { createContext, useContext } from 'react'
import type { AuthActionResult, AuthState } from '@mycopilot/host-api'

export interface AccountAuthContextValue {
  state: AuthState
  loginRequested: boolean
  requestLogin(): void
  dismissLogin(): void
  canStartTurn(): boolean
  logout(): Promise<AuthActionResult>
}

export const AccountAuthContext = createContext<AccountAuthContextValue | null>(null)
export function useAccountAuth(): AccountAuthContextValue | null {
  return useContext(AccountAuthContext)
}
