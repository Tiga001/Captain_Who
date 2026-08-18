import {
  MAX_STORED_RUN_ITEMS,
  hasExactKeys,
  hasOwn,
  isApprovalStatus,
  isBoundedString,
  isNullableBoundedString,
  isNullableSafeInteger,
  isOptionalBoundedString,
  isOptionalSafeInteger,
  isRecord,
  isRecordArray,
  isSafeInteger,
  isStringArray
} from './persistedAgentRunValidation'

const MAX_OFFICE_RENDER_PAGES = 128
const MAX_OFFICE_PAGE_NUMBER = 10_000
const MAX_OFFICE_SCREENSHOT_DIMENSION = 16_384
const MAX_OFFICE_GRID_COLUMNS = 32
const MAX_U32 = 0xffff_ffff

function isFileInputRef(value: unknown): boolean {
  if (!isRecord(value) || typeof value.type !== 'string') return false
  if (
    value.type === 'attachment' &&
    hasExactKeys(value, ['type', 'readPath']) &&
    isBoundedString(value.readPath, 16 * 1024)
  ) {
    return true
  }
  if (
    (value.type === 'workspace' || value.type === 'external') &&
    hasExactKeys(value, ['type', 'path']) &&
    isBoundedString(value.path, 16 * 1024)
  ) {
    return true
  }
  if (
    value.type === 'generated_artifact' &&
    hasExactKeys(value, ['type', 'uri', 'path']) &&
    isBoundedString(value.uri, 16 * 1024) &&
    isBoundedString(value.path, 16 * 1024)
  ) {
    return true
  }
  return (
    value.type === 'skill_resource' &&
    hasExactKeys(value, ['type', 'uri']) &&
    isBoundedString(value.uri, 16 * 1024)
  )
}

function isFileInputSpec(value: unknown): boolean {
  return (
    isRecord(value) &&
    hasExactKeys(value, ['mountPath', 'source']) &&
    isBoundedString(value.mountPath, 16 * 1024) &&
    isFileInputRef(value.source)
  )
}

function isFileInputBinding(value: unknown): boolean {
  return (
    isRecord(value) &&
    hasExactKeys(value, ['schemaVersion', 'mountPath', 'source', 'sizeBytes', 'sha256']) &&
    value.schemaVersion === 1 &&
    isBoundedString(value.mountPath, 16 * 1024) &&
    isFileInputRef(value.source) &&
    isSafeInteger(value.sizeBytes) &&
    typeof value.sha256 === 'string' &&
    /^[0-9a-f]{64}$/u.test(value.sha256)
  )
}

function isOfficePropertyMap(value: unknown): boolean {
  if (!isRecord(value)) return false
  return Object.values(value).every(
    (property) =>
      typeof property === 'string' ||
      typeof property === 'number' ||
      typeof property === 'boolean' ||
      (isRecord(property) &&
        hasExactKeys(property, ['resourcePath']) &&
        isBoundedString(property.resourcePath, 16 * 1024))
  )
}

function isOfficePosition(value: unknown): boolean {
  return (
    isRecord(value) &&
    ((value.type === 'index' &&
      hasExactKeys(value, ['type', 'index']) &&
      isSafeInteger(value.index)) ||
      ((value.type === 'after' || value.type === 'before') &&
        hasExactKeys(value, ['type', 'target']) &&
        isBoundedString(value.target, 16 * 1024)))
  )
}

function isOfficeParameters(value: unknown): boolean {
  if (!isRecord(value) || typeof value.type !== 'string') return false
  switch (value.type) {
    case 'help':
      return (
        hasExactKeys(value, ['type'], ['verb', 'element']) &&
        (!hasOwn(value, 'verb') ||
          [
            'status',
            'help',
            'create',
            'view',
            'get',
            'query',
            'validate',
            'set',
            'add',
            'remove',
            'move',
            'swap'
          ].includes(value.verb as string)) &&
        isOptionalBoundedString(value, 'element', 1024)
      )
    case 'create':
      return (
        hasExactKeys(value, ['type'], ['locale', 'minimal', 'overwrite']) &&
        isOptionalBoundedString(value, 'locale', 1024) &&
        (!hasOwn(value, 'minimal') || typeof value.minimal === 'boolean') &&
        (!hasOwn(value, 'overwrite') || typeof value.overwrite === 'boolean')
      )
    case 'view': {
      if (
        !hasExactKeys(
          value,
          ['type', 'mode'],
          [
            'start',
            'end',
            'maxLines',
            'issueType',
            'limit',
            'columns',
            'pages',
            'range',
            'viewport',
            'grid',
            'renderMode',
            'pageCount'
          ]
        ) ||
        ![
          'text',
          'annotated',
          'outline',
          'stats',
          'issues',
          'html',
          'svg',
          'screenshot',
          'forms'
        ].includes(value.mode as string) ||
        !isOptionalSafeInteger(value, 'start') ||
        !isOptionalSafeInteger(value, 'end') ||
        !isOptionalSafeInteger(value, 'maxLines') ||
        !isOptionalBoundedString(value, 'issueType', 1024) ||
        !isOptionalSafeInteger(value, 'limit') ||
        (hasOwn(value, 'columns') && !isStringArray(value.columns, 1024)) ||
        !isOptionalBoundedString(value, 'range', 16 * 1024) ||
        (hasOwn(value, 'renderMode') &&
          value.renderMode !== 'auto' &&
          value.renderMode !== 'html') ||
        (hasOwn(value, 'pageCount') && typeof value.pageCount !== 'boolean')
      ) {
        return false
      }
      if (
        hasOwn(value, 'pages') &&
        !isRecordArray(value.pages, (page) =>
          Boolean(
            hasExactKeys(page, ['start'], ['end']) &&
            isSafeInteger(page.start) &&
            isOptionalSafeInteger(page, 'end')
          )
        )
      ) {
        return false
      }
      if (
        hasOwn(value, 'viewport') &&
        (!isRecord(value.viewport) ||
          !hasExactKeys(value.viewport, ['width', 'height']) ||
          !isSafeInteger(value.viewport.width, 1) ||
          !isSafeInteger(value.viewport.height, 1))
      ) {
        return false
      }
      if (hasOwn(value, 'grid')) {
        if (!isRecord(value.grid)) return false
        if (!(
          (value.grid.mode === 'auto' && hasExactKeys(value.grid, ['mode'])) ||
          (value.grid.mode === 'columns' &&
            hasExactKeys(value.grid, ['mode', 'columns']) &&
            isSafeInteger(value.grid.columns, 1))
        )) {
          return false
        }
      }
      return true
    }
    case 'get':
      return (
        hasExactKeys(value, ['type'], ['target', 'depth']) &&
        isOptionalBoundedString(value, 'target', 16 * 1024) &&
        isOptionalSafeInteger(value, 'depth')
      )
    case 'query':
      return (
        hasExactKeys(value, ['type', 'selector'], ['contains', 'compact', 'fields']) &&
        isBoundedString(value.selector, 16 * 1024) &&
        isOptionalBoundedString(value, 'contains', 16 * 1024, true) &&
        (!hasOwn(value, 'compact') || typeof value.compact === 'boolean') &&
        (!hasOwn(value, 'fields') || isStringArray(value.fields, 1024))
      )
    case 'validate':
      return hasExactKeys(value, ['type'])
    case 'set':
      return (
        hasExactKeys(value, ['type', 'target'], ['properties', 'replacement', 'force']) &&
        isBoundedString(value.target, 16 * 1024) &&
        (!hasOwn(value, 'properties') || isOfficePropertyMap(value.properties)) &&
        (!hasOwn(value, 'replacement') ||
          (isRecord(value.replacement) &&
            hasExactKeys(value.replacement, ['find', 'replace']) &&
            isBoundedString(value.replacement.find, 128 * 1024, true) &&
            isBoundedString(value.replacement.replace, 128 * 1024, true))) &&
        (!hasOwn(value, 'force') || typeof value.force === 'boolean')
      )
    case 'add':
      return (
        hasExactKeys(
          value,
          ['type', 'parent', 'elementType'],
          ['copyFrom', 'position', 'properties', 'force']
        ) &&
        isBoundedString(value.parent, 16 * 1024) &&
        isBoundedString(value.elementType, 1024) &&
        isOptionalBoundedString(value, 'copyFrom', 16 * 1024) &&
        (!hasOwn(value, 'position') || isOfficePosition(value.position)) &&
        (!hasOwn(value, 'properties') || isOfficePropertyMap(value.properties)) &&
        (!hasOwn(value, 'force') || typeof value.force === 'boolean')
      )
    case 'remove':
      return (
        hasExactKeys(value, ['type', 'target'], ['shift', 'properties']) &&
        isBoundedString(value.target, 16 * 1024) &&
        (!hasOwn(value, 'shift') || value.shift === 'left' || value.shift === 'up') &&
        (!hasOwn(value, 'properties') || isOfficePropertyMap(value.properties))
      )
    case 'move':
      return (
        hasExactKeys(value, ['type', 'target'], ['newParent', 'position', 'properties']) &&
        isBoundedString(value.target, 16 * 1024) &&
        isOptionalBoundedString(value, 'newParent', 16 * 1024) &&
        (!hasOwn(value, 'position') || isOfficePosition(value.position)) &&
        (!hasOwn(value, 'properties') || isOfficePropertyMap(value.properties))
      )
    case 'swap':
      return (
        hasExactKeys(value, ['type', 'firstTarget', 'secondTarget']) &&
        isBoundedString(value.firstTarget, 16 * 1024) &&
        isBoundedString(value.secondTarget, 16 * 1024)
      )
    default:
      return false
  }
}

function isOfficeRequest(value: unknown): boolean {
  return (
    isRecord(value) &&
    hasExactKeys(value, [
      'documentKind',
      'operation',
      'documentPath',
      'outputPath',
      'destinationPath',
      'inputs',
      'timeoutMs',
      'parameters'
    ]) &&
    ['document', 'spreadsheet', 'presentation'].includes(value.documentKind as string) &&
    [
      'help',
      'create',
      'view',
      'get',
      'query',
      'validate',
      'set',
      'add',
      'remove',
      'move',
      'swap'
    ].includes(value.operation as string) &&
    isNullableBoundedString(value.documentPath, 16 * 1024) &&
    isNullableBoundedString(value.outputPath, 16 * 1024) &&
    isNullableBoundedString(value.destinationPath, 16 * 1024) &&
    Array.isArray(value.inputs) &&
    value.inputs.length <= MAX_STORED_RUN_ITEMS &&
    value.inputs.every(isFileInputSpec) &&
    isNullableSafeInteger(value.timeoutMs) &&
    isOfficeParameters(value.parameters) &&
    isRecord(value.parameters) &&
    value.parameters.type === value.operation
  )
}

function isOfficePathSlot(value: unknown): boolean {
  return (
    isRecord(value) &&
    (((value.type === 'document' || value.type === 'output' || value.type === 'destination') &&
      hasExactKeys(value, ['type'])) ||
      (value.type === 'resource' &&
        hasExactKeys(value, ['type', 'index']) &&
        isSafeInteger(value.index)))
  )
}

function isOfficePathIdentity(value: unknown): boolean {
  return (
    isRecord(value) &&
    hasExactKeys(value, ['revision', 'device', 'inode']) &&
    isBoundedString(value.revision, 1024) &&
    isNullableSafeInteger(value.device) &&
    isNullableSafeInteger(value.inode)
  )
}

function isOfficeFrozenPath(value: unknown): boolean {
  return (
    isRecord(value) &&
    hasExactKeys(value, [
      'slot',
      'logicalPath',
      'purpose',
      'scope',
      'normalizedPath',
      'state',
      'objectIdentity',
      'parentIdentity',
      'contentRevision',
      'size',
      'writeDisposition'
    ]) &&
    isOfficePathSlot(value.slot) &&
    isBoundedString(value.logicalPath, 16 * 1024) &&
    ['readSource', 'writeTarget', 'inPlaceTarget'].includes(value.purpose as string) &&
    ['workspace', 'external', 'attachment'].includes(value.scope as string) &&
    isBoundedString(value.normalizedPath, 16 * 1024) &&
    (value.state === 'missing' || value.state === 'present') &&
    (value.objectIdentity === null || isOfficePathIdentity(value.objectIdentity)) &&
    isOfficePathIdentity(value.parentIdentity) &&
    isNullableBoundedString(value.contentRevision, 1024) &&
    isNullableSafeInteger(value.size) &&
    (value.writeDisposition === null ||
      value.writeDisposition === 'createNew' ||
      value.writeDisposition === 'replaceExisting')
  )
}

function isOfficePrepared(value: unknown): boolean {
  return (
    isRecord(value) &&
    hasExactKeys(value, [
      'schemaVersion',
      'providerId',
      'engineRevision',
      'workspaceRevision',
      'access',
      'request',
      'argv',
      'resolvedRenderPlan',
      'paths',
      'inputBindings'
    ]) &&
    value.schemaVersion === 6 &&
    isBoundedString(value.providerId, 1024) &&
    isBoundedString(value.engineRevision, 4096) &&
    isNullableBoundedString(value.workspaceRevision, 4096) &&
    (value.access === 'readOnly' || value.access === 'fileWrite') &&
    isOfficeRequest(value.request) &&
    isStringArray(value.argv, 64 * 1024) &&
    (value.resolvedRenderPlan === null ||
      isOfficePresentationRenderPlan(value.resolvedRenderPlan)) &&
    (value.resolvedRenderPlan !== null) === requiresPresentationRenderPlan(value.request) &&
    Array.isArray(value.paths) &&
    value.paths.length <= MAX_STORED_RUN_ITEMS &&
    value.paths.every(isOfficeFrozenPath) &&
    Array.isArray(value.inputBindings) &&
    value.inputBindings.length <= MAX_STORED_RUN_ITEMS &&
    value.inputBindings.every(isFileInputBinding)
  )
}

function requiresPresentationRenderPlan(value: unknown): boolean {
  if (!isRecord(value) || !isRecord(value.parameters)) return false
  return (
    value.documentKind === 'presentation' &&
    value.operation === 'view' &&
    value.parameters.type === 'view' &&
    value.parameters.mode === 'screenshot'
  )
}

function isOfficePresentationRenderPlan(value: unknown): boolean {
  if (
    !isRecord(value) ||
    !hasExactKeys(value, [
      'requestedPages',
      'slideWidthEmu',
      'slideHeightEmu',
      'viewport',
      'grid'
    ]) ||
    !Array.isArray(value.requestedPages) ||
    value.requestedPages.length === 0 ||
    value.requestedPages.length > MAX_OFFICE_RENDER_PAGES ||
    !value.requestedPages.every(
      (page, index, pages) =>
        isSafeInteger(page) &&
        page > 0 &&
        page <= MAX_OFFICE_PAGE_NUMBER &&
        (index === 0 || (pages[index - 1] as number) < page)
    ) ||
    !isSafeInteger(value.slideWidthEmu) ||
    value.slideWidthEmu <= 0 ||
    value.slideWidthEmu > MAX_U32 ||
    !isSafeInteger(value.slideHeightEmu) ||
    value.slideHeightEmu <= 0 ||
    value.slideHeightEmu > MAX_U32 ||
    !isRecord(value.viewport) ||
    !hasExactKeys(value.viewport, ['width', 'height']) ||
    !isSafeInteger(value.viewport.width) ||
    value.viewport.width <= 0 ||
    value.viewport.width > MAX_OFFICE_SCREENSHOT_DIMENSION ||
    !isSafeInteger(value.viewport.height) ||
    value.viewport.height <= 0 ||
    value.viewport.height > MAX_OFFICE_SCREENSHOT_DIMENSION
  ) {
    return false
  }
  return (
    value.grid === null ||
    (isRecord(value.grid) &&
      hasExactKeys(value.grid, ['mode', 'columns']) &&
      value.grid.mode === 'columns' &&
      isSafeInteger(value.grid.columns) &&
      value.grid.columns > 0 &&
      value.grid.columns <= MAX_OFFICE_GRID_COLUMNS)
  )
}

export function isOfficeOperation(value: unknown): boolean {
  return (
    isRecord(value) &&
    hasExactKeys(value, [
      'schemaVersion',
      'id',
      'semanticArgs',
      'prepared',
      'approvalStatus',
      'reason'
    ]) &&
    value.schemaVersion === 6 &&
    isBoundedString(value.id, 1024) &&
    isRecord(value.semanticArgs) &&
    isOfficePrepared(value.prepared) &&
    isApprovalStatus(value.approvalStatus) &&
    isBoundedString(value.reason, 16 * 1024, true)
  )
}
