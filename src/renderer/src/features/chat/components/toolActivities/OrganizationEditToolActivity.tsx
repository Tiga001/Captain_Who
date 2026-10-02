import { Building2, ChevronLeft, ChevronRight } from 'lucide-react'
import { useState } from 'react'
import { BotGroupIcon } from '../../../../components/BotGroupIcon'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import {
  record,
  records,
  text,
  type RecordValue,
  type WorkflowQueryProps
} from './workflowQueryPresentation'
import './WorkflowQueryToolActivity.css'
import './OrganizationEditToolActivity.css'

const fieldLabels: Record<string, [string, string]> = {
  name: ['名称', 'Name'],
  task: ['任务', 'Task'],
  receives: ['接收内容', 'Receives'],
  delivers: ['交付内容', 'Delivers'],
  rank: ['职级', 'Rank'],
  managementRole: ['管理角色', 'Management role'],
  departmentId: ['所属部门', 'Department'],
  modelConfigId: ['模型', 'Model'],
  permissionMode: ['权限', 'Permissions'],
  parentId: ['上级部门', 'Parent department']
}
const managementRoles: Record<string, [string, string]> = {
  member: ['普通成员', 'Member'],
  organization_admin: ['组织管理员', 'Organization administrator'],
  department_admin: ['部门管理员', 'Department administrator']
}
const permissionModes: Record<string, [string, string]> = {
  default: ['默认权限', 'Default permissions'],
  custom: ['自定义权限', 'Custom permissions'],
  full: ['完全权限', 'Full permissions']
}
const actionLabels: Record<string, [string, string]> = {
  add_member: ['添加成员', 'Added member'],
  update_member: ['更新成员', 'Updated member'],
  remove_member: ['移除成员', 'Removed member'],
  add_department: ['添加部门', 'Added department'],
  update_department: ['更新部门', 'Updated department'],
  remove_department: ['移除部门', 'Removed department']
}

/** References use receipt-time display names, never raw model or department IDs. */
function fieldValue(field: RecordValue, side: 'before' | 'after', chinese: boolean): string {
  const key = text(field.field)
  const value = field[side]
  const pick = (labels: [string, string]) => labels[chinese ? 0 : 1]
  if (['departmentId', 'parentId', 'modelConfigId'].includes(key)) {
    if (value === null || value === undefined)
      return key === 'modelConfigId'
        ? pick(['未选择模型', 'No model selected'])
        : pick(['直属组织', 'Directly under the organization'])
    return text(field[`${side}Label`]).trim() || pick(['名称暂不可用', 'Name unavailable'])
  }
  if (value === null || value === undefined || value === '') return pick(['未设置', 'Not set'])
  if (key === 'managementRole')
    return pick(managementRoles[text(value)] ?? ['未知角色', 'Unknown role'])
  if (key === 'permissionMode')
    return pick(permissionModes[text(value)] ?? ['未知权限', 'Unknown permissions'])
  if (key === 'rank')
    return typeof value === 'number' && Number.isFinite(value)
      ? String(value)
      : pick(['未知职级', 'Unknown rank'])
  return typeof value === 'string' ? value : pick(['内容已更新', 'Content updated'])
}

/** Each kind of change keeps its own selection while the disclosure is collapsed. */
function OrganizationEditChangeGroup({
  changes,
  department,
  chinese
}: {
  changes: RecordValue[]
  department: boolean
  chinese: boolean
}) {
  const [selectedIndex, setSelectedIndex] = useState(0)
  const currentIndex = Math.max(0, Math.min(selectedIndex, changes.length - 1))
  // Receipts normally remain immutable. Reconcile a shorter refreshed receipt before painting.
  if (selectedIndex !== currentIndex) setSelectedIndex(currentIndex)
  const change = changes[currentIndex]
  if (!change) return null

  const pick = (labels: [string, string]) => labels[chinese ? 0 : 1]
  const Icon = department ? Building2 : BotGroupIcon
  const name =
    text(change.entityName).trim() || pick(department ? ['部门', 'Department'] : ['成员', 'Member'])
  const fields = records(change.fields).filter((field) =>
    Object.hasOwn(fieldLabels, text(field.field))
  )
  const previousLabel = pick(
    department ? ['上一项', 'Previous department'] : ['上一位', 'Previous member']
  )
  const nextLabel = pick(department ? ['下一项', 'Next department'] : ['下一位', 'Next member'])

  return (
    <section className="organization-edit-change">
      <div className="organization-edit-change__heading">
        <Icon aria-hidden="true" />
        <strong>
          {pick(actionLabels[text(change.action)])} · {name}
        </strong>
        {changes.length > 1 && (
          <nav
            className="organization-edit-change__navigation"
            aria-label={pick(
              department
                ? ['切换部门变更', 'Department changes']
                : ['切换成员变更', 'Member changes']
            )}
          >
            <button
              type="button"
              aria-label={previousLabel}
              title={previousLabel}
              disabled={currentIndex === 0}
              onClick={() => setSelectedIndex(currentIndex - 1)}
            >
              <ChevronLeft aria-hidden="true" />
            </button>
            <span aria-atomic="true" aria-live="polite">
              {currentIndex + 1} / {changes.length}
            </span>
            <button
              type="button"
              aria-label={nextLabel}
              title={nextLabel}
              disabled={currentIndex === changes.length - 1}
              onClick={() => setSelectedIndex(currentIndex + 1)}
            >
              <ChevronRight aria-hidden="true" />
            </button>
          </nav>
        )}
      </div>
      {fields.length > 0 && (
        <dl className="organization-edit-change__fields">
          {fields.map((field, fieldIndex) => (
            <div key={fieldIndex}>
              <dt>{pick(fieldLabels[text(field.field)])}</dt>
              <dd>
                <span className="organization-edit-change__before">
                  {fieldValue(field, 'before', chinese)}
                </span>
                <span
                  className="organization-edit-change__arrow"
                  aria-label={pick(['改为', 'changed to'])}
                >
                  →
                </span>
                <span>{fieldValue(field, 'after', chinese)}</span>
              </dd>
            </div>
          ))}
        </dl>
      )}
    </section>
  )
}

/** Organization edit receipts show member and department changes without execution metadata. */
export function OrganizationEditToolActivity(props: WorkflowQueryProps) {
  const { language } = useFrontendConfig()
  const chinese = language.startsWith('zh')
  const pick = (labels: [string, string]) => labels[chinese ? 0 : 1]
  const status = props.result
    ? props.result.ok
      ? 'completed'
      : 'failed'
    : props.cancelled || props.settledStatus === 'cancelled'
      ? 'cancelled'
      : props.settledStatus === 'failed'
        ? 'failed'
        : props.settledStatus === 'completed'
          ? 'unknown'
          : 'running'
  const data = record(props.result?.result)
  const changes = records(data.changes).filter(
    (change) =>
      (change.entityType === 'member' || change.entityType === 'department') &&
      Object.hasOwn(actionLabels, text(change.action))
  )
  const organization = text(data.organizationName).trim() || pick(['组织', 'organization'])
  const label =
    status === 'running'
      ? pick([`正在编辑${organization}`, `Editing ${organization}`])
      : status === 'completed'
        ? changes.length
          ? pick([`组织变动【${organization}】`, `Organization changes [${organization}]`])
          : pick(['组织配置未发生变化', 'No organization changes'])
        : status === 'cancelled'
          ? pick(['已取消组织编辑', 'Organization edit cancelled'])
          : status === 'failed'
            ? pick(['组织编辑失败', 'Organization edit failed'])
            : pick(['组织编辑结果待确认', 'Organization edit result is unconfirmed'])
  const reason = text(record(props.call.args).reason).trim()
  return (
    <AgentActivityDisclosure
      icon={BotGroupIcon}
      label={reason ? `${label} · ${reason}` : label}
      isPending={status === 'running'}
      hasDetails={status === 'completed' && changes.length > 0}
      className="agent-activity--organization-edit"
    >
      {status === 'completed' && (
        <div className="workflow-query organization-edit-changes">
          <OrganizationEditChangeGroup
            key={`${props.call.id}:members`}
            changes={changes.filter((change) => change.entityType === 'member')}
            department={false}
            chinese={chinese}
          />
          <OrganizationEditChangeGroup
            key={`${props.call.id}:departments`}
            changes={changes.filter((change) => change.entityType === 'department')}
            department
            chinese={chinese}
          />
        </div>
      )}
    </AgentActivityDisclosure>
  )
}
