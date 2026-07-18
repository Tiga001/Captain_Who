import { HostInvocationError } from '@mycopilot/host-api'
import type { SkillActivationErrorData, SkillSelection } from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import {
  planSkillActivationRecovery,
  reconcileSkillActivationSelections
} from '../skillActivationRecovery'

const firstSelection: SkillSelection = {
  id: 'workspace:project-a:first',
  revision: 'skill-sha256-v1:first'
}
const secondSelection: SkillSelection = {
  id: 'bundled:application:second',
  revision: 'skill-sha256-v1:second'
}

function activationError(overrides: Partial<SkillActivationErrorData>) {
  return new HostInvocationError({
    message: 'Skill activation failed',
    data: {
      type: 'skillActivation',
      code: 'sourceUnavailable',
      recovery: 'retrySameSelection',
      message: 'Skill activation failed',
      ...overrides
    }
  })
}

describe('Skill activation recovery planning', () => {
  it('removes only the failed selection when rejection identifies a submitted Skill', () => {
    expect(
      planSkillActivationRecovery(
        activationError({
          code: 'invalidSelection',
          recovery: 'rejectSelection',
          skillId: firstSelection.id
        }),
        [firstSelection, secondSelection]
      )
    ).toEqual({
      draftPolicy: 'rejectSelection',
      refreshCatalog: false,
      rejectedSkillId: firstSelection.id,
      selectionsToRestore: [secondSelection]
    })
  })

  it('restores no selection when a rejection cannot be bound to submitted input', () => {
    expect(
      planSkillActivationRecovery(
        activationError({
          code: 'tooManySkills',
          recovery: 'rejectSelection',
          skillId: undefined
        }),
        [firstSelection, secondSelection]
      )
    ).toEqual({
      draftPolicy: 'discardSubmitted',
      refreshCatalog: false,
      selectionsToRestore: []
    })
  })

  it('preserves submitted selections and explicitly requests a catalog refresh', () => {
    expect(
      planSkillActivationRecovery(activationError({ code: 'stale', recovery: 'refreshCatalog' }), [
        firstSelection,
        secondSelection
      ])
    ).toEqual({
      draftPolicy: 'restoreMissing',
      refreshCatalog: true,
      selectionsToRestore: [firstSelection, secondSelection]
    })
  })

  it('preserves submitted selections for an explicit retry policy or ordinary failure', () => {
    expect(
      planSkillActivationRecovery(activationError({ recovery: 'retrySameSelection' }), [
        firstSelection
      ])
    ).toEqual({
      draftPolicy: 'restoreMissing',
      refreshCatalog: false,
      selectionsToRestore: [firstSelection]
    })
    expect(planSkillActivationRecovery(new Error('transport failed'), [firstSelection])).toEqual({
      draftPolicy: 'restoreMissing',
      refreshCatalog: false,
      selectionsToRestore: [firstSelection]
    })
  })

  it('fails closed when claimed activation recovery data is malformed', () => {
    const error = new HostInvocationError({
      message: 'Malformed activation failure',
      data: {
        type: 'skillActivation',
        code: 'stale',
        recovery: 'futureRecovery',
        message: 'Malformed activation failure'
      }
    })

    expect(planSkillActivationRecovery(error, [firstSelection])).toEqual({
      draftPolicy: 'discardSubmitted',
      refreshCatalog: false,
      selectionsToRestore: []
    })
  })

  it('keeps a newer live revision instead of restoring the stale submitted revision', () => {
    const latestSelection = {
      ...firstSelection,
      revision: 'skill-sha256-v1:latest'
    }
    const plan = planSkillActivationRecovery(
      activationError({ code: 'stale', recovery: 'refreshCatalog' }),
      [firstSelection, secondSelection]
    )

    expect(reconcileSkillActivationSelections([latestSelection], plan)).toEqual([
      latestSelection,
      secondSelection
    ])
  })

  it('removes an explicitly rejected id even when it was re-selected during the request', () => {
    const reselected = {
      ...firstSelection,
      revision: 'skill-sha256-v1:reselected'
    }
    const liveOnlySelection: SkillSelection = {
      id: 'workspace:project-a:live-only',
      revision: 'skill-sha256-v1:live-only'
    }
    const plan = planSkillActivationRecovery(
      activationError({
        code: 'invalidSelection',
        recovery: 'rejectSelection',
        skillId: firstSelection.id
      }),
      [firstSelection, secondSelection]
    )

    expect(reconcileSkillActivationSelections([reselected, liveOnlySelection], plan)).toEqual([
      liveOnlySelection,
      secondSelection
    ])
  })
})
