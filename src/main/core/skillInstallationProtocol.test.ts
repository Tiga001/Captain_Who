import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

import type {
  SkillActivationErrorData,
  SkillAcquisitionSource,
  SkillsListInput
} from '@mycopilot/protocol'
import {
  parseSkillInspectionErrorData,
  parseSkillInstallationCommitOutput,
  parseSkillInstallationPreview,
  parseSkillManagementErrorData,
  parseSkillPreparationCancellationOutput,
  parseSkillsChangedNotification,
  parseSkillsListManagementOutput,
  parseSkillsSetEnabledOutput,
  SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION,
  SKILL_MANAGEMENT_ERROR_CODE,
  SKILL_MANAGEMENT_SCHEMA_VERSION,
  SKILLS_CANCEL_PREPARATION_METHOD,
  SKILLS_CHANGED_NOTIFICATION_METHOD,
  SKILLS_COMMIT_INSTALLATION_METHOD,
  SKILLS_INSPECT_INSTALLATION_METHOD,
  SKILLS_LIST_MANAGEMENT_METHOD,
  SKILLS_SET_ENABLED_METHOD
} from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'

interface InstallationWorkflowGolden {
  workflowSchemaVersion: number
  managementSchemaVersion: number
  inspectCases: Array<{ name: string; request: unknown; preview: unknown }>
  commit: { request: unknown; response: unknown }
  cancel: { request: unknown; response: unknown }
  management: {
    listRequest: unknown
    listResponse: unknown
    setEnabledRequest: unknown
    setEnabledResponse: unknown
    changed: unknown
  }
  inspectionError: unknown
  managementErrors: unknown[]
}

const golden = JSON.parse(
  readFileSync(
    resolve(process.cwd(), 'packages/protocol/fixtures/skill-installation-workflow-v1.json'),
    'utf8'
  )
) as InstallationWorkflowGolden

describe('Skill installation workflow protocol', () => {
  it('keeps the stable method and schema identifiers', () => {
    expect(SKILLS_INSPECT_INSTALLATION_METHOD).toBe('skills.inspectInstallation')
    expect(SKILLS_COMMIT_INSTALLATION_METHOD).toBe('skills.commitInstallation')
    expect(SKILLS_CANCEL_PREPARATION_METHOD).toBe('skills.cancelPreparation')
    expect(SKILLS_LIST_MANAGEMENT_METHOD).toBe('skills.listManagement')
    expect(SKILLS_SET_ENABLED_METHOD).toBe('skills.setEnabled')
    expect(SKILLS_CHANGED_NOTIFICATION_METHOD).toBe('skills.changed')
    expect(golden.workflowSchemaVersion).toBe(SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION)
    expect(golden.managementSchemaVersion).toBe(SKILL_MANAGEMENT_SCHEMA_VERSION)
    expect(SKILL_MANAGEMENT_ERROR_CODE).toBe(-32012)
  })

  it('parses the same workflow golden serialized by Rust', () => {
    for (const inspectCase of golden.inspectCases) {
      expect(parseSkillInstallationPreview(inspectCase.preview)).toEqual(inspectCase.preview)
    }
    expect(parseSkillInstallationCommitOutput(golden.commit.response)).toEqual(
      golden.commit.response
    )
    expect(parseSkillPreparationCancellationOutput(golden.cancel.response)).toEqual(
      golden.cancel.response
    )
    expect(parseSkillsListManagementOutput(golden.management.listResponse)).toEqual(
      golden.management.listResponse
    )
    expect(parseSkillsSetEnabledOutput(golden.management.setEnabledResponse)).toEqual(
      golden.management.setEnabledResponse
    )
    expect(parseSkillsChangedNotification(golden.management.changed)).toEqual(
      golden.management.changed
    )
    expect(parseSkillInspectionErrorData(golden.inspectionError)).toEqual(golden.inspectionError)
    for (const error of golden.managementErrors) {
      expect(parseSkillManagementErrorData(error)).toEqual(error)
    }
  })

  it('models every acquisition source and permits catalog listing without a project', () => {
    const sources = golden.inspectCases.map(
      (inspectCase) => (inspectCase.request as { source: SkillAcquisitionSource }).source
    )
    expect(sources.map((source) => source.kind)).toEqual([
      'localDirectory',
      'githubRepository',
      'installedSource'
    ])

    const globalList = {} satisfies SkillsListInput
    expect(globalList).toEqual({})
  })

  it('models disabled activation as a stable reject-selection failure', () => {
    const error = {
      type: 'skillActivation',
      code: 'disabled',
      recovery: 'rejectSelection',
      message: 'The selected Skill is disabled.',
      skillId: 'installed:user:22222222-2222-4222-8222-222222222222'
    } satisfies SkillActivationErrorData

    expect(error.code).toBe('disabled')
    expect(error.recovery).toBe('rejectSelection')
  })

  it('rejects inconsistent or malformed untrusted responses', () => {
    expect(() =>
      parseSkillInstallationCommitOutput({
        ...(golden.commit.response as Record<string, unknown>),
        operation: 'install',
        outcome: 'updated'
      })
    ).toThrow('outcome updated is invalid for install')

    const preview = golden.inspectCases[0].preview as Record<string, unknown>
    expect(() =>
      parseSkillInstallationPreview({
        ...preview,
        package: { ...(preview.package as Record<string, unknown>), fileCount: 0 }
      })
    ).toThrow('fileCount')

    expect(() =>
      parseSkillsChangedNotification({
        ...(golden.management.changed as Record<string, unknown>),
        reason: 'preparationChanged'
      })
    ).toThrow('reason')

    expect(() =>
      parseSkillManagementErrorData({
        type: 'skillManagement',
        operation: 'setEnabled',
        code: 'stale',
        recovery: 'refreshManagement',
        message: 'Refresh the management inventory.'
      })
    ).toThrow('code')

    expect(() =>
      parseSkillManagementErrorData({
        ...(golden.managementErrors[0] as Record<string, unknown>),
        internalStoragePath: '/private/state.db'
      })
    ).toThrow('unexpected field internalStoragePath')
  })
})
