import {
  expectString,
  invalidProtocolValue,
  expectRecord,
  expectOnlyKeys,
  expectEnum,
  expectArray
} from '../skills/validation'
import type {
  StorageProjectFolderInput,
  StorageProjectCreateInput,
  StorageProjectUpdateInput,
  StorageProjectValidationErrorData
} from '../storage'
import { MAX_PROJECT_FOLDERS } from '../storage'

const PROJECT_FOLDER_ROLES = ['primary', 'auxiliary'] as const

const PROJECT_VALIDATION_CODES = [
  'name_required',
  'folders_required',
  'primary_required',
  'too_many_folders',
  'folder_missing',
  'folder_duplicate',
  'folder_nested',
  'project_missing'
] as const

const MAX_PROJECT_NAME_BYTES = 512

const MAX_PROJECT_FOLDER_PATH_BYTES = 16_384

const MAX_PROJECT_IDENTIFIER_BYTES = 128

function expectBoundedString(value: unknown, context: string, maxBytes: number): string {
  const text = expectString(value, context)
  if (new TextEncoder().encode(text).byteLength > maxBytes) {
    throw invalidProtocolValue(context, `must not exceed ${maxBytes} UTF-8 bytes`)
  }
  return text
}

function parseStorageProjectFolderInput(
  value: unknown,
  context: string
): StorageProjectFolderInput {
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['id', 'path', 'role'] as const, context)
  const path = expectBoundedString(record.path, `${context}.path`, MAX_PROJECT_FOLDER_PATH_BYTES)
  if (path.trim().length === 0) {
    throw invalidProtocolValue(`${context}.path`, 'expected a non-empty string')
  }
  const folder: StorageProjectFolderInput = {
    path,
    role: expectEnum(record.role, PROJECT_FOLDER_ROLES, `${context}.role`)
  }
  if (record.id !== undefined && record.id !== null) {
    folder.id = expectBoundedString(record.id, `${context}.id`, MAX_PROJECT_IDENTIFIER_BYTES)
    if (folder.id.trim().length === 0) {
      throw invalidProtocolValue(`${context}.id`, 'expected a non-empty string')
    }
  }
  return folder
}

function parseStorageProjectFolderInputs(
  value: unknown,
  context: string
): StorageProjectFolderInput[] {
  const folders = expectArray(value, context)
  if (folders.length > MAX_PROJECT_FOLDERS) {
    throw invalidProtocolValue(context, `must not exceed ${MAX_PROJECT_FOLDERS} folders`)
  }
  return folders.map((folder, index) =>
    parseStorageProjectFolderInput(folder, `${context}[${index}]`)
  )
}

/** Validates the renderer's create request shape; directory existence is checked by Main. */
export function parseStorageProjectCreateInput(value: unknown): StorageProjectCreateInput {
  const context = 'storage project create input'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['name', 'folders'] as const, context)
  return {
    name: expectBoundedString(record.name, `${context}.name`, MAX_PROJECT_NAME_BYTES),
    folders: parseStorageProjectFolderInputs(record.folders, `${context}.folders`)
  }
}

/** Validates the renderer's update request shape; directory existence is checked by Main. */
export function parseStorageProjectUpdateInput(value: unknown): StorageProjectUpdateInput {
  const context = 'storage project update input'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['projectId', 'name', 'folders'] as const, context)
  const projectId = expectBoundedString(
    record.projectId,
    `${context}.projectId`,
    MAX_PROJECT_IDENTIFIER_BYTES
  )
  if (projectId.trim().length === 0) {
    throw invalidProtocolValue(`${context}.projectId`, 'expected a non-empty string')
  }
  return {
    projectId,
    name: expectBoundedString(record.name, `${context}.name`, MAX_PROJECT_NAME_BYTES),
    folders: parseStorageProjectFolderInputs(record.folders, `${context}.folders`)
  }
}

export function parseStorageProjectValidationErrorData(
  value: unknown
): StorageProjectValidationErrorData {
  const context = 'storage project validation error data'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['kind', 'code', 'path'] as const, context)
  const data: StorageProjectValidationErrorData = {
    kind: expectEnum(record.kind, ['project_validation'] as const, `${context}.kind`),
    code: expectEnum(record.code, PROJECT_VALIDATION_CODES, `${context}.code`)
  }
  if (record.path !== undefined) {
    data.path = expectBoundedString(record.path, `${context}.path`, MAX_PROJECT_FOLDER_PATH_BYTES)
  }
  return data
}
