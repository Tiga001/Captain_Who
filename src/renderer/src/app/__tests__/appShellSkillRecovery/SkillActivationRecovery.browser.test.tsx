import { HostInvocationError } from '@mycopilot/host-api'
import { describe, expect, it } from 'vitest'
import type { ChatComposerDraft } from '../../../features/chat/chatTypes'
import { testState } from './state'
import { skillSelection, latestSkillSelection } from './turnFixtures'
import { deferred } from './eventFixtures'
import { renderSelectedConversation } from './setup'

describe('activation failure recovery', () => {
  it('does not restore a selection rejected by the host', async () => {
    testState.startConversationTurn.mockRejectedValueOnce(
      new HostInvocationError({
        message: 'Selection was rejected',
        data: {
          type: 'skillActivation',
          code: 'invalidSelection',
          recovery: 'rejectSelection',
          message: 'Selection was rejected',
          skillId: skillSelection.id
        }
      })
    )

    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-with-skill' }).click()

    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    await expect
      .element(screen.getByTestId('draft-skills'))
      .not.toHaveTextContent(skillSelection.id)
    expect(
      (testState.saveComposerDraft.mock.calls.at(-1)?.[1] as ChatComposerDraft | undefined)?.skills
    ).toEqual([])
    await expect.element(screen.getByTestId('skill-catalog-refresh-token')).toHaveTextContent('0')
  })

  it('restores the selection and explicitly requests a catalog refresh', async () => {
    testState.startConversationTurn.mockRejectedValueOnce(
      new HostInvocationError({
        message: 'Selection is stale',
        data: {
          type: 'skillActivation',
          code: 'stale',
          recovery: 'refreshCatalog',
          message: 'Selection is stale',
          skillId: skillSelection.id,
          expectedRevision: skillSelection.revision,
          actualRevision: 'skill-sha256-v1:latest'
        }
      })
    )

    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-with-skill' }).click()

    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    await expect.element(screen.getByTestId('draft-skills')).toHaveTextContent(skillSelection.id)
    await expect.element(screen.getByTestId('skill-catalog-refresh-token')).toHaveTextContent('1')
  })

  it('does not replace a newer live revision when a deferred request reports stale input', async () => {
    const start = deferred<never>()
    testState.startConversationTurn.mockReturnValueOnce(start.promise)

    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-with-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await screen.getByRole('button', { name: 'select-latest-skill' }).click()
    await expect
      .element(screen.getByTestId('draft-skills'))
      .toHaveTextContent(latestSkillSelection.revision)

    start.reject(
      new HostInvocationError({
        message: 'Selection is stale',
        data: {
          type: 'skillActivation',
          code: 'stale',
          recovery: 'refreshCatalog',
          message: 'Selection is stale',
          skillId: skillSelection.id,
          expectedRevision: skillSelection.revision,
          actualRevision: latestSkillSelection.revision
        }
      })
    )

    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    await expect
      .element(screen.getByTestId('draft-skills'))
      .toHaveTextContent(skillSelection.revision)
    expect(
      JSON.parse(screen.getByTestId('queued-message-payloads').element().textContent!)
    ).toEqual([expect.objectContaining({ skills: [latestSkillSelection], status: 'pending' })])
  })

  it('removes a rejected Skill that was re-selected while the request was in flight', async () => {
    const start = deferred<never>()
    testState.startConversationTurn.mockReturnValueOnce(start.promise)

    const screen = await renderSelectedConversation()
    await screen.getByRole('button', { name: 'submit-with-skill' }).click()
    await expect.poll(() => testState.startConversationTurn.mock.calls.length).toBe(1)
    await screen.getByRole('button', { name: 'select-latest-skill' }).click()
    await expect.element(screen.getByTestId('draft-skills')).toHaveTextContent(skillSelection.id)

    start.reject(
      new HostInvocationError({
        message: 'Selection was rejected',
        data: {
          type: 'skillActivation',
          code: 'invalidSelection',
          recovery: 'rejectSelection',
          message: 'Selection was rejected',
          skillId: skillSelection.id
        }
      })
    )

    await expect.element(screen.getByTestId('last-assistant-status')).toHaveTextContent('sent')
    await expect.element(screen.getByTestId('agent-run-status')).toHaveTextContent('completed')
    await expect
      .element(screen.getByTestId('draft-skills'))
      .not.toHaveTextContent(skillSelection.id)
  })
})
