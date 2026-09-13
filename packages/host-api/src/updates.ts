/** Public projection only: never contains a feed URL, local path, credentials or release notes. */
export interface UpdateState {
  revision: number
  status: 'disabled' | 'checking' | 'idle' | 'available' | 'downloading' | 'installing' | 'error'
  version: string | null
  percent: number
  error: 'checkFailed' | 'downloadFailed' | 'installFailed' | null
}

export interface UpdateHostApi {
  getState(): Promise<UpdateState>
  download(): Promise<UpdateState>
  onStateChanged(handler: (state: UpdateState) => void): () => void
}
