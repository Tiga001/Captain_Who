// Renderer image-generation activity: presents one safe, callId-stable card for every Tool call.
import {
  CircleHelp,
  CircleSlash2,
  ImageOff,
  Images,
  LoaderCircle,
  type LucideIcon
} from 'lucide-react'
import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import type { TranslationKey } from '../../../../config/frontendTranslations'
import {
  getImageGenerationActivityView,
  type ImageGenerationActivityOperation,
  type ImageGenerationActivityStatus
} from '../../../imageGeneration/imageGenerationActivity'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import type { SettledToolStatus } from './toolActivityUtils'

const LABEL_KEY: Readonly<
  Record<ImageGenerationActivityOperation, Record<ImageGenerationActivityStatus, TranslationKey>>
> = {
  generate: {
    running: 'agent.imageGeneration.generate.running',
    completed: 'agent.imageGeneration.generate.completed',
    failed: 'agent.imageGeneration.generate.failed',
    cancelled: 'agent.imageGeneration.generate.cancelled',
    outcomeIndeterminate: 'agent.imageGeneration.generate.outcomeIndeterminate',
    commitIndeterminate: 'agent.imageGeneration.generate.commitIndeterminate'
  },
  edit: {
    running: 'agent.imageGeneration.edit.running',
    completed: 'agent.imageGeneration.edit.completed',
    failed: 'agent.imageGeneration.edit.failed',
    cancelled: 'agent.imageGeneration.edit.cancelled',
    outcomeIndeterminate: 'agent.imageGeneration.edit.outcomeIndeterminate',
    commitIndeterminate: 'agent.imageGeneration.edit.commitIndeterminate'
  },
  unknown: {
    running: 'agent.imageGeneration.unknown.running',
    completed: 'agent.imageGeneration.unknown.completed',
    failed: 'agent.imageGeneration.unknown.failed',
    cancelled: 'agent.imageGeneration.unknown.cancelled',
    outcomeIndeterminate: 'agent.imageGeneration.unknown.outcomeIndeterminate',
    commitIndeterminate: 'agent.imageGeneration.unknown.commitIndeterminate'
  }
}

const STATUS_ICON: Readonly<Record<ImageGenerationActivityStatus, LucideIcon>> = {
  running: LoaderCircle,
  completed: Images,
  failed: ImageOff,
  cancelled: CircleSlash2,
  outcomeIndeterminate: CircleHelp,
  commitIndeterminate: CircleHelp
}

export function ImageGenerationToolActivity({
  call,
  result,
  settledStatus
}: {
  call: AgentToolCall
  result?: AgentToolResult
  settledStatus?: SettledToolStatus
}) {
  const { t } = useFrontendConfig()
  const view = getImageGenerationActivityView(call, result, settledStatus)
  const Icon = STATUS_ICON[view.status]
  const safeFailure = view.status === 'failed' ? t('agent.imageGeneration.safeFailure') : undefined
  const details = [view.reason, view.failureMessage ?? safeFailure, view.failureRecovery].filter(
    (value, index, values): value is string => Boolean(value && values.indexOf(value) === index)
  )

  return (
    <AgentActivityDisclosure
      className="agent-activity--image-generation"
      hasDetails={details.length > 0}
      icon={Icon}
      isPending={view.status === 'running'}
      label={t(LABEL_KEY[view.operation][view.status])}
    >
      {details.length > 0 ? (
        <div className="agent-activity__details image-generation-activity__details">
          {details.map((detail) => (
            <p key={detail}>{detail}</p>
          ))}
        </div>
      ) : null}
    </AgentActivityDisclosure>
  )
}
