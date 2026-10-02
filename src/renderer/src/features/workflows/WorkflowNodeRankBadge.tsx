import { Badge, Circle } from 'lucide-react'
import type { WorkflowNode } from '@mycopilot/protocol'
import type { WorkflowText } from './workflowText'

export function WorkflowNodeRankBadge({ node, text }: { node: WorkflowNode; text: WorkflowText }) {
  const rank = node.rank ?? 1
  const isAdmin =
    node.managementRole === 'organization_admin' || node.managementRole === 'department_admin'
  const roleLabel = text(
    node.managementRole === 'organization_admin'
      ? 'roleOrganizationAdmin'
      : node.managementRole === 'department_admin'
        ? 'roleDepartmentAdmin'
        : 'roleMember'
  )
  const label = `${roleLabel} · ${text('rank')} ${rank}`
  const Shape = isAdmin ? Badge : Circle

  return (
    <span
      className={`workflow-node__rank${isAdmin ? ' is-admin' : ''}`}
      role="img"
      aria-label={label}
      title={label}
    >
      <Shape size={23} strokeWidth={1.6} aria-hidden="true" />
      <span aria-hidden="true">{rank}</span>
    </span>
  )
}
