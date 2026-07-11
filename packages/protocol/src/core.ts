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
