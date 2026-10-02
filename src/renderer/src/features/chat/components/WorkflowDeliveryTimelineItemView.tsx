import { useState } from 'react'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import type { ChatWorkflowDeliveryTimelineItem } from '../chatTypes'
import type { WorkspaceReferenceTarget } from '../workspaceMentions'
import { ChatMarkdown } from './ChatMarkdown'
import { ChatMessageActions } from './ChatMessageActions'
import { WorkflowReceivedContent, workflowReceivedCopyText } from './WorkflowReceivedContent'
import './WorkflowDeliveryTimelineItemView.css'

export function WorkflowDeliveryTimelineItemView({
  item,
  onOpenWorkspaceReference,
  projectId
}: {
  item: ChatWorkflowDeliveryTimelineItem
  onOpenWorkspaceReference?: (target: WorkspaceReferenceTarget) => void
  projectId?: string | null
}) {
  const { language } = useFrontendConfig()
  const [expanded, setExpanded] = useState(false)
  const bodies = item.sources.map((source) => ({
    nodeName: source.nodeName || source.conversationTitle,
    content: source.content
  }))
  return (
    <section
      className="workflow-delivery"
      data-workflow-input-id={item.inputId}
      data-trace-sequence={item.traceSequence}
    >
      <div className="chat-message__input-origin" data-input-origin="workflow">
        <span>{language.startsWith('zh') ? '来自组织' : 'From organization'}</span>
        <strong>{item.workflowName}</strong>
        <span>· {bodies.map((body) => body.nodeName).join('、')}</span>
      </div>
      <div className="workflow-delivery__bubble">
        {expanded ? (
          <ChatMarkdown
            enableMath={false}
            content={item.content}
            onOpenWorkspaceReference={onOpenWorkspaceReference}
            projectId={projectId}
          />
        ) : (
          <WorkflowReceivedContent
            bodies={bodies}
            onOpenWorkspaceReference={onOpenWorkspaceReference}
            projectId={projectId}
          />
        )}
      </div>
      <ChatMessageActions
        content={expanded ? item.content : workflowReceivedCopyText(bodies)}
        onWorkflowContextToggle={() => setExpanded((value) => !value)}
        showTokenUsageDetails={false}
        timestamp={item.createdAt}
        workflowContextExpanded={expanded}
      />
    </section>
  )
}
