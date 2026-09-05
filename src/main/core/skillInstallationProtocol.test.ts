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
  inspectionErrors: unknown[]
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
    for (const error of golden.inspectionErrors) {
      expect(parseSkillInspectionErrorData(error)).toEqual(error)
    }
    for (const error of golden.managementErrors) {
      expect(parseSkillManagementErrorData(error)).toEqual(error)
    }
  })

  it('accepts the actionable image configuration requirement without widening error fields', () => {
    const error = {
      type: 'skillManagement',
      operation: 'setEnabled',
      code: 'configurationRequired',
      recovery: 'configureImageGeneration',
      message: 'Configure image generation before enabling its Skill.'
    }
    expect(parseSkillManagementErrorData(error)).toEqual(error)
    expect(() => parseSkillManagementErrorData({ ...error, credential: 'private' })).toThrow(
      'unexpected field credential'
    )
  })

  it('accepts the current unsupported tool reference diagnostic and rejects drift', () => {
    const response = structuredClone(golden.management.listResponse) as {
      diagnostics: unknown[]
    }
    response.diagnostics = [
      {
        code: 'unsupportedToolReference',
        severity: 'warning',
        message: 'The Skill references a model tool that is not available.',
        location: '.agents/skills/example/SKILL.md'
      }
    ]
    expect(parseSkillsListManagementOutput(response).diagnostics).toEqual(response.diagnostics)

    const unknownCode = structuredClone(response) as { diagnostics: Array<Record<string, unknown>> }
    unknownCode.diagnostics[0].code = 'futureDiagnostic'
    expect(() => parseSkillsListManagementOutput(unknownCode)).toThrow(/Skill diagnostic\.code/)

    const extraField = structuredClone(response) as { diagnostics: Array<Record<string, unknown>> }
    extraField.diagnostics[0].internalCause = 'must not cross the protocol boundary'
    expect(() => parseSkillsListManagementOutput(extraField)).toThrow(
      /unexpected field internalCause/
    )
  })

  it('preserves Host image enablement eligibility and rejects unknown block reasons', () => {
    const response = structuredClone(golden.management.listResponse) as {
      skills: Array<Record<string, unknown>>
    }
    response.skills[0].enablementBlock = 'imageGenerationConfigurationRequired'
    expect(parseSkillsListManagementOutput(response).skills[0].enablementBlock).toBe(
      'imageGenerationConfigurationRequired'
    )
    response.skills[0].enablementBlock = 'untrustedFutureReason'
    expect(() => parseSkillsListManagementOutput(response)).toThrow(/enablementBlock/)
  })

  it('keeps the golden aligned with backend installation identity and source semantics', () => {
    const localInstall = golden.inspectCases.find((testCase) => testCase.name === 'local install')
    const githubUpdate = golden.inspectCases.find((testCase) => testCase.name === 'GitHub update')
    const installedSourceUpdate = golden.inspectCases.find(
      (testCase) => testCase.name === 'update from installed source'
    )
    if (!localInstall || !githubUpdate || !installedSourceUpdate) {
      throw new Error('The workflow fixture must cover all current acquisition paths')
    }

    const localRequest = localInstall.request as { preparationId: string }
    const localPreview = localInstall.preview as {
      installationId: string
      previewRevision: string
      source: { kind: string; refreshable: boolean }
    }
    expect(localPreview.installationId).toBe(localRequest.preparationId)
    expect(localPreview.source).toMatchObject({ kind: 'localDirectory', refreshable: false })

    for (const testCase of golden.inspectCases) {
      const preview = testCase.preview as {
        compatibility: { issues: Array<{ code: string }>; status: string }
        expiresAtUnixMs: number
        package: { fileCount: number }
        previewRevision: string
      }
      expect(preview.previewRevision).toMatch(/^skill-install-preview-sha256-v1:[0-9a-f]{64}$/)
      expect(preview.expiresAtUnixMs).toBe(4102444800000)
      if (preview.package.fileCount > 1) {
        expect(preview.compatibility.status).toBe('compatibleWithWarnings')
        expect(preview.compatibility.issues).toContainEqual(
          expect.objectContaining({ code: 'resourcesNotExposed' })
        )
      }
    }

    const githubPreview = githubUpdate.preview as {
      compatibility: { issues: Array<{ id: string; code: string }> }
    }
    expect(githubPreview.compatibility.issues).toContainEqual(
      expect.objectContaining({ id: 'containsScripts', code: 'containsScripts' })
    )
    expect((golden.commit.request as { acceptedIssueIds: string[] }).acceptedIssueIds).toEqual([
      'containsScripts'
    ])

    expect((installedSourceUpdate.request as { source: { kind: string } }).source.kind).toBe(
      'installedSource'
    )
    const installedSourcePreview = installedSourceUpdate.preview as {
      compatibility: { status: string }
      source: { kind: string }
    }
    expect(installedSourcePreview.source.kind).toBe('githubRepository')
    expect(installedSourcePreview.compatibility.status).toBe('compatibleWithWarnings')

    const management = golden.management.listResponse as {
      managementRevision: string
      skills: Array<{
        compatibility: { issues: unknown[]; status: string }
        source: { kind: string }
        stateRevision: string
      }>
    }
    expect(management.managementRevision).toMatch(/^skill-management-catalog-sha256-v2:/)
    expect(
      management.skills.every((skill) =>
        /^skill-management-state-sha256-v2:/.test(skill.stateRevision)
      )
    ).toBe(true)
    expect(
      management.skills.find((skill) => skill.source.kind === 'installed')?.compatibility
    ).toEqual({
      status: 'unknown',
      issues: []
    })

    const sourceNotRefreshable = golden.inspectionErrors
      .map(parseSkillInspectionErrorData)
      .find((error) => error.code === 'sourceNotRefreshable')
    expect(sourceNotRefreshable).toMatchObject({
      recovery: 'refreshManagement',
      message: 'This installation does not have a supported refresh source.'
    })

    const commitIndeterminate = golden.inspectionErrors
      .map(parseSkillInspectionErrorData)
      .find((error) => error.code === 'commitIndeterminate')
    expect(commitIndeterminate?.skillId).toBe((githubUpdate.preview as { skillId: string }).skillId)
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
