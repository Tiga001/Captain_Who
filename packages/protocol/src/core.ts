// Protocol layer.
export const CORE_PING_METHOD = 'core.ping'
export const CORE_SHUTDOWN_METHOD = 'core.shutdown'
export const APP_GET_VERSION_METHOD = 'app.getVersion'

export interface CorePingRequest {
  message?: string
}

export interface CorePingResponse {
  message: 'pong'
  echo?: string
  serverTimeMs: number
}

export interface CoreShutdownResponse {
  cancelledRuns: number
  timedOut: boolean
}

export interface AppVersionResponse {
  name: string
  version: string
}
