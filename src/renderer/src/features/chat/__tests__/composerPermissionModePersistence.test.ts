import { describe, expect, it } from 'vitest'
import {
  CURRENT_COMPOSER_PERMISSION_MODE_VERSION,
  normalizeStoredComposerPermissionMode,
  serializeComposerPermissionMode
} from '../../storage/composerPermissionModePersistence'

describe('normalizeStoredComposerPermissionMode', () => {
  it.each([undefined, null, 0, 1, 3])(
    'downgrades full permission stored with legacy or unknown version %s',
    (permissionModeVersion) => {
      expect(normalizeStoredComposerPermissionMode('full', permissionModeVersion)).toBe('default')
    }
  )

  it('preserves full only when it was selected under the current semantics', () => {
    expect(
      normalizeStoredComposerPermissionMode('full', CURRENT_COMPOSER_PERMISSION_MODE_VERSION)
    ).toBe('full')
  })

  it('keeps non-escalating supported modes independent of the version marker', () => {
    expect(normalizeStoredComposerPermissionMode('default', undefined)).toBe('default')
    expect(normalizeStoredComposerPermissionMode('custom', undefined)).toBe('custom')
  })

  it('fails closed for unsupported stored values', () => {
    expect(normalizeStoredComposerPermissionMode('legacy-full', 1)).toBe('default')
  })
})

describe('serializeComposerPermissionMode', () => {
  it.each(['default', 'custom', 'full'] as const)(
    'stamps a newly saved %s choice with the current semantics version',
    (permissionMode) => {
      expect(serializeComposerPermissionMode(permissionMode)).toEqual({
        permissionMode,
        permissionModeVersion: CURRENT_COMPOSER_PERMISSION_MODE_VERSION
      })
    }
  )
})
