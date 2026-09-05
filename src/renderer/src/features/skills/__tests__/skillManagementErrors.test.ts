import { HostInvocationError } from '@mycopilot/host-api'
import { describe, expect, it } from 'vitest'
import {
  getSkillOperationErrorDetails,
  getSkillOperationErrorKey
} from '../management/skillManagementErrors'

function skillError(data: Record<string, unknown>): HostInvocationError {
  return new HostInvocationError({
    message: 'private backend detail that must not reach the UI',
    data
  })
}

describe('skill management error presentation', () => {
  it('maps structured failures to localized presentation keys', () => {
    const details = getSkillOperationErrorDetails(
      skillError({
        type: 'skillSourceResolution',
        code: 'networkUnavailable',
        recovery: 'retrySameResolution'
      })
    )

    expect(details).toEqual({
      code: 'networkUnavailable',
      commitMayHaveSucceeded: false,
      intendedInstallationRevision: undefined,
      kind: 'sourceResolution',
      phase: undefined,
      recovery: 'retrySameResolution',
      skillId: undefined
    })
    expect(getSkillOperationErrorKey(details)).toBe('skills.error.networkUnavailable')
  })

  it('prioritizes indeterminate commits and structured recovery actions', () => {
    expect(
      getSkillOperationErrorKey(
        getSkillOperationErrorDetails(
          skillError({
            type: 'skillInstallation',
            code: 'commitIndeterminate',
            commitMayHaveSucceeded: true
          })
        )
      )
    ).toBe('skills.operationNeedsConfirmation')

    expect(
      getSkillOperationErrorKey(
        getSkillOperationErrorDetails(
          skillError({
            type: 'skillInstallation',
            code: 'capacityExceeded',
            recovery: 'freeCapacity'
          })
        )
      )
    ).toBe('skills.freeCapacityDescription')
  })

  it('does not retain raw error text for unknown failures', () => {
    const details = getSkillOperationErrorDetails(new Error('sensitive transport detail'))

    expect(details).toEqual({ commitMayHaveSucceeded: false, kind: 'unknown' })
    expect(getSkillOperationErrorKey(details)).toBe('skills.operationFailed')
    expect(details).not.toHaveProperty('message')
  })

  it('explains where to configure image generation without showing backend details', () => {
    const details = getSkillOperationErrorDetails(
      skillError({
        type: 'skillManagement',
        operation: 'setEnabled',
        code: 'configurationRequired',
        recovery: 'configureImageGeneration'
      })
    )
    expect(details.kind).toBe('management')
    expect(getSkillOperationErrorKey(details)).toBe('skills.error.configurationRequired')
    expect(details).not.toHaveProperty('message')
  })
})
