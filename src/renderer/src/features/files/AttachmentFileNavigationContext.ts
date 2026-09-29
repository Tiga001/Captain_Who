import { createContext } from 'react'
import type { AttachmentFileRequest } from '@mycopilot/protocol'

export interface AttachmentFileNavigationTarget extends AttachmentFileRequest {
  name: string
}

/** Only interactive chat surfaces use this ordinary attachment capability. */
export const AttachmentFileNavigationContext = createContext<
  ((target: AttachmentFileNavigationTarget) => void) | null
>(null)
