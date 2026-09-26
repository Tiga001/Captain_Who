import type { ChatMessage } from '../chatTypes'
import type { WorkspaceReferenceTarget } from '../workspaceMentions'
import { ChatMarkdown } from './ChatMarkdown'
import './WorkflowReceivedContent.css'

export interface WorkflowReceivedBody {
  nodeName: string
  content: string
}

export function workflowReceivedBodies(message: ChatMessage): WorkflowReceivedBody[] | null {
  if (message.role !== 'user' || message.humanInteractionDisplay) return null
  const sources = message.workflowSource?.sources
  // The Host supplies the original source bodies. Never guess boundaries in an assembled message
  // or silently hide content from a legacy record that lacks complete provenance.
  if (!sources?.length || !sources.every((source) => typeof source.content === 'string')) {
    return null
  }
  return sources.map((source) => ({ nodeName: source.nodeName, content: source.content! }))
}

export function workflowReceivedCopyText(bodies: WorkflowReceivedBody[]): string {
  return bodies.length === 1
    ? bodies[0].content
    : bodies.map((body) => `${body.nodeName}\n${body.content}`).join('\n\n')
}

export function WorkflowReceivedContent({
  bodies,
  onOpenWorkspaceReference,
  projectId
}: {
  bodies: WorkflowReceivedBody[]
  onOpenWorkspaceReference?: (target: WorkspaceReferenceTarget) => void
  projectId?: string | null
}) {
  return (
    <div className="workflow-received-content">
      {bodies.map((body, index) => (
        <section className="workflow-received-content__message" key={index}>
          {bodies.length > 1 && (
            <div className="workflow-received-content__source">{body.nodeName}</div>
          )}
          <ChatMarkdown
            enableMath={false}
            content={body.content}
            onOpenWorkspaceReference={onOpenWorkspaceReference}
            projectId={projectId}
          />
        </section>
      ))}
    </div>
  )
}
