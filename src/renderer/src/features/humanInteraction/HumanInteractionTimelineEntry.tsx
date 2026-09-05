import { MessageCircleQuestionMark } from 'lucide-react'
import type { HumanInteractionRequestSnapshot } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import type { HumanInteractionControllerView } from './useHumanInteraction'
import './HumanInteractionHistory.css'

export type HumanInteractionTimelineController = Pick<
  HumanInteractionControllerView,
  'openRequests' | 'canInteract' | 'open'
>

export function HumanInteractionTimelineEntry({
  request,
  interaction
}: {
  request: HumanInteractionRequestSnapshot
  interaction: HumanInteractionTimelineController
}) {
  const { t } = useFrontendConfig()
  return (
    <div className="human-interaction-entries">
      <button
        className="human-interaction-entry"
        data-human-request-id={request.requestId}
        disabled={
          !interaction.canInteract || interaction.openRequests.some((item) => item.mode === 'sync')
        }
        onClick={() => interaction.open(request.requestId)}
        type="button"
      >
        <MessageCircleQuestionMark aria-hidden="true" />
        {t('humanInteraction.timeline.answer').replace('{count}', String(request.questions.length))}
      </button>
    </div>
  )
}
