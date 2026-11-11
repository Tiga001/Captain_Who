export type JsonRpcId = string | number

export interface JsonRpcRequest<TParams = unknown> {
  jsonrpc: '2.0'
  id: JsonRpcId
  method: string
  params?: TParams
}

export interface JsonRpcNotification<TParams = unknown> {
  jsonrpc: '2.0'
  method: string
  params?: TParams
}

export interface JsonRpcErrorObject {
  code: number
  message: string
  data?: unknown
}

export interface JsonRpcSuccessResponse<TResult = unknown> {
  jsonrpc: '2.0'
  id: JsonRpcId
  result: TResult
}

export interface JsonRpcErrorResponse {
  jsonrpc: '2.0'
  id: JsonRpcId | null
  error: JsonRpcErrorObject
}

export type JsonRpcResponse<TResult = unknown> =
  JsonRpcSuccessResponse<TResult> | JsonRpcErrorResponse

/** Validate the transport envelope before correlating a response or dispatching a notification. */
export function isJsonRpcServerMessage(
  value: unknown
): value is JsonRpcResponse | JsonRpcNotification {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return false
  const message = value as Record<string, unknown>
  if (message.jsonrpc !== '2.0') return false
  if ('method' in message) {
    return (
      typeof message.method === 'string' &&
      message.method.length > 0 &&
      !('id' in message) &&
      !('result' in message) &&
      !('error' in message)
    )
  }
  if (!('id' in message) || 'result' in message === 'error' in message) return false
  if (
    message.id !== null &&
    typeof message.id !== 'string' &&
    (typeof message.id !== 'number' || !Number.isSafeInteger(message.id))
  )
    return false
  if ('result' in message) return message.id !== null
  const error = message.error
  return (
    typeof error === 'object' &&
    error !== null &&
    !Array.isArray(error) &&
    'code' in error &&
    typeof error.code === 'number' &&
    Number.isSafeInteger(error.code) &&
    'message' in error &&
    typeof error.message === 'string'
  )
}
