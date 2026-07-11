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
