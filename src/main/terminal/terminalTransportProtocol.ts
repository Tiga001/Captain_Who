import type {
  TerminalCreateSessionRequest,
  TerminalExitEvent,
  TerminalOutputEvent
} from '@mycopilot/protocol'

import type { TerminalProjectSources } from './terminalSourceDirectories'

export type TerminalTrustedCreateSessionRequest = TerminalCreateSessionRequest & {
  projectSources?: TerminalProjectSources
}

export type TerminalServiceRequest =
  | {
      id: number
      method: 'terminal.selectSourceDirectory'
      params: { sessionId: string; folderId: string }
      type: 'request'
    }
  | {
      id: number
      method: 'terminal.createSession'
      params: TerminalTrustedCreateSessionRequest
      type: 'request'
    }
  | {
      id: number
      method: 'terminal.resizeSession'
      params: {
        cols: number
        rows: number
        sessionId: string
      }
      type: 'request'
    }
  | {
      id: number
      method: 'terminal.killSession'
      params: {
        sessionId: string
      }
      type: 'request'
    }
  | {
      id: number
      method: 'terminal.shutdown'
      type: 'request'
    }

export type TerminalServiceCommand =
  | { method: 'terminal.markUserInput'; params: { sessionId: string }; type: 'command' }
  | {
      method: 'terminal.writeInput'
      params: {
        data: string
        userInitiated?: boolean
        sessionId: string
      }
      type: 'command'
    }
  | {
      method: 'terminal.acknowledgeOutput'
      params: {
        sequence: number
        sessionId: string
      }
      type: 'command'
    }
  | {
      method: 'terminal.disposeSession'
      params: {
        sessionId: string
      }
      type: 'command'
    }

export type TerminalServiceInboundMessage = TerminalServiceCommand | TerminalServiceRequest

export type TerminalServiceResponse = {
  error?: string
  id: number
  result?: unknown
  success: boolean
  type: 'response'
}

export type TerminalServiceNotification =
  | {
      event: TerminalOutputEvent
      method: 'terminal.output'
      type: 'notification'
    }
  | {
      event: TerminalExitEvent
      method: 'terminal.exit'
      type: 'notification'
    }

export type TerminalServiceOutboundMessage = TerminalServiceNotification | TerminalServiceResponse
