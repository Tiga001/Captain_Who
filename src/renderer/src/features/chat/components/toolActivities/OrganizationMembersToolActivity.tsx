import { OrganizationEditToolActivity } from './OrganizationEditToolActivity'
import { record, records, type WorkflowQueryProps } from './workflowQueryPresentation'

/** Presentation-only adapter for saved receipts; this does not expose an executable tool alias. */
export function OrganizationMembersToolActivity(props: WorkflowQueryProps) {
  const data = record(props.result?.result)
  const changes = records(data.changes).map((change) => ({
    action:
      change.action === 'add'
        ? 'add_member'
        : change.action === 'remove'
          ? 'remove_member'
          : 'update_member',
    entityType: 'member',
    entityName: change.nodeName,
    fields:
      change.action === 'set_rank'
        ? [{ field: 'rank', before: change.previousRank, after: change.rank }]
        : []
  }))
  return (
    <OrganizationEditToolActivity
      {...props}
      result={props.result ? { ...props.result, result: { ...data, changes } } : undefined}
    />
  )
}
