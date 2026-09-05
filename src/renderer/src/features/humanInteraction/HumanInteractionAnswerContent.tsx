import type { HumanInteractionResponseDisplay } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import './HumanInteractionHistory.css'

export function HumanInteractionAnswerContent({
  display
}: {
  display: HumanInteractionResponseDisplay
}) {
  const { t } = useFrontendConfig()
  return (
    <div className="human-interaction-answer" data-human-response-id={display.responseId}>
      {display.answers.map((answer) => (
        <div className="human-interaction-answer__pair" key={answer.questionId}>
          <div className="human-interaction-answer__question">{answer.question}</div>
          <div className="human-interaction-answer__value">
            {answer.kind === 'skipped' ? t('humanInteraction.history.skipped') : answer.answer}
          </div>
        </div>
      ))}
    </div>
  )
}
