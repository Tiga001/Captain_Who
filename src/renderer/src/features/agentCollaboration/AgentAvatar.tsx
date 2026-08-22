import { getAgentAvatarIndex, getAgentAvatarUrl } from './agentAvatarAssignment'
import './AgentAvatar.css'

interface AgentAvatarProps {
  agentId: string
  className?: string
}

/** Decorative avatar shared by every child-Agent entry point. */
export function AgentAvatar({ agentId, className }: AgentAvatarProps) {
  const avatarIndex = getAgentAvatarIndex(agentId)
  return (
    <span
      aria-hidden="true"
      className={['agent-avatar', className].filter(Boolean).join(' ')}
      data-agent-avatar-index={avatarIndex}
    >
      <img alt="" draggable={false} src={getAgentAvatarUrl(agentId)} />
    </span>
  )
}
