import { useState } from 'react'
import lightBoat from '../../../../../resources/brand-mark-light.png'
import darkBoat from '../../../../../resources/brand-mark-dark.png'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'

export function AccountAvatar({ src }: { src?: string | null }) {
  const { resolvedColorScheme } = useFrontendConfig()
  const [failedSource, setFailedSource] = useState<string | null>(null)
  const fallback = resolvedColorScheme === 'dark' ? darkBoat : lightBoat
  const source = src && src !== failedSource ? src : fallback
  return (
    <img
      src={source}
      alt=""
      onError={() => {
        if (src) setFailedSource(src)
      }}
    />
  )
}
