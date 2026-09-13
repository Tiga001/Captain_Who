import { createContext, useContext } from 'react'
import type { LicenseState } from '@mycopilot/host-api'

export interface LicenseContextValue {
  state: LicenseState
  canStartTurn(): boolean
  requestAccess(): void
  handleDenied?(error: unknown): boolean
  refresh(): Promise<LicenseState>
}

export const LicenseContext = createContext<LicenseContextValue | null>(null)
export function useLicense(): LicenseContextValue | null {
  return useContext(LicenseContext)
}
