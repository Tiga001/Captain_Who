import { useState } from 'react'
import lightBoat from '../../../../../resources/brand-mark-light.png'
import darkBoat from '../../../../../resources/brand-mark-dark.png'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { getAgentAvatarUrl } from '../agentCollaboration/agentAvatarAssignment'

export function AccountAvatar({
  src,
  localAvatarSeed
}: {
  src?: string | null
  localAvatarSeed?: string
}) {
  const { resolvedColorScheme } = useFrontendConfig()
  const [failedSource, setFailedSource] = useState<string | null>(null)
  const fallback = resolvedColorScheme === 'dark' ? darkBoat : lightBoat
  const avatarSource = localAvatarSeed ? getAgentAvatarUrl(localAvatarSeed) : src
  const source = avatarSource && avatarSource !== failedSource ? avatarSource : fallback
  return (
    <img
      src={source}
      alt=""
      onError={() => {
        if (avatarSource) setFailedSource(avatarSource)
      }}
    />
  )
}
