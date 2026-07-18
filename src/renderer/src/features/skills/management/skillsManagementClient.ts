// Renderer skills management client: checks structured Host API results before exposing values.
import { HostInvocationError } from '@mycopilot/host-api'
import type {
  SkillInstallationCommitOutput,
  SkillInstallationPreview,
  SkillMutationOutput,
  SkillPreparationCancellationOutput,
  SkillsCancelSourceResolutionInput,
  SkillsCancelSourceResolutionOutput,
  SkillsCancelPreparationInput,
  SkillsCommitInstallationInput,
  SkillsInspectInstallationInput,
  SkillsListManagementOutput,
  SkillsResolveInstallationSourceInput,
  SkillsResolveInstallationSourceOutput,
  SkillsSetEnabledInput,
  SkillsSetEnabledOutput,
  SkillsUninstallInput
} from '@mycopilot/protocol'
import { hostClient } from '../../../host/hostClient'

export async function listManagedSkills(): Promise<SkillsListManagementOutput> {
  const result = await hostClient.skills.listManagement({})
  if (!result.ok) throw new HostInvocationError(result.error)
  return result.value
}

export function selectSkillInstallationDirectory(): Promise<string | null> {
  return hostClient.skills.selectInstallationDirectory()
}

export async function resolveSkillInstallationSource(
  input: SkillsResolveInstallationSourceInput
): Promise<SkillsResolveInstallationSourceOutput> {
  const result = await hostClient.skills.resolveInstallationSource(input)
  if (!result.ok) throw new HostInvocationError(result.error)
  return result.value
}

export async function cancelSkillSourceResolution(
  input: SkillsCancelSourceResolutionInput
): Promise<SkillsCancelSourceResolutionOutput> {
  const result = await hostClient.skills.cancelSourceResolution(input)
  if (!result.ok) throw new HostInvocationError(result.error)
  return result.value
}

export async function inspectSkillInstallation(
  input: SkillsInspectInstallationInput
): Promise<SkillInstallationPreview> {
  const result = await hostClient.skills.inspectInstallation(input)
  if (!result.ok) throw new HostInvocationError(result.error)
  return result.value
}

export async function commitSkillInstallation(
  input: SkillsCommitInstallationInput
): Promise<SkillInstallationCommitOutput> {
  const result = await hostClient.skills.commitInstallation(input)
  if (!result.ok) throw new HostInvocationError(result.error)
  return result.value
}

export async function cancelSkillPreparation(
  input: SkillsCancelPreparationInput
): Promise<SkillPreparationCancellationOutput> {
  const result = await hostClient.skills.cancelPreparation(input)
  if (!result.ok) throw new HostInvocationError(result.error)
  return result.value
}

export async function setManagedSkillEnabled(
  input: SkillsSetEnabledInput
): Promise<SkillsSetEnabledOutput> {
  const result = await hostClient.skills.setEnabled(input)
  if (!result.ok) throw new HostInvocationError(result.error)
  return result.value
}

export async function uninstallManagedSkill(
  input: SkillsUninstallInput
): Promise<SkillMutationOutput> {
  const result = await hostClient.skills.uninstall(input)
  if (!result.ok) throw new HostInvocationError(result.error)
  return result.value
}

export const onManagedSkillsChanged = hostClient.skills.onChanged
