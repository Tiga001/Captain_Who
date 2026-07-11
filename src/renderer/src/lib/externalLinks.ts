import { hostClient } from '../host/hostClient'

export function openExternalUrl(url: string): Promise<void> {
  return hostClient.app.openExternal(url)
}
