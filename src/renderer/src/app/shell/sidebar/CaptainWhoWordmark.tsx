import { useId } from 'react'
import wordmarkMaskSource from '../../../assets/captain-who-wordmark.png'
import './CaptainWhoWordmark.css'

const SOURCE_WIDTH = 2172
const SOURCE_HEIGHT = 724

// These source-pixel clips only separate colors; the original PNG alpha remains the sole shape.
const WHO_WITHOUT_ACCENTS_PATH = [
  'M 1202 0 H 2172 V 724 H 1202 Z',
  'M 1914 182 H 1978 V 290 H 1914 Z',
  'M 1978 214 H 2062 V 308 H 1978 Z',
  'M 1968 290 H 1978 V 308 H 1968 Z',
  'M 1997 308 H 2091 V 353 H 1997 Z',
  'M 2062 303 H 2091 V 308 H 2062 Z'
].join(' ')

export function CaptainWhoWordmark() {
  const instanceId = useId().replace(/:/g, '')
  const alphaMaskId = `captain-who-alpha-${instanceId}`
  const captainClipId = `captain-who-captain-${instanceId}`
  const whoClipId = `captain-who-who-${instanceId}`
  const accentClipId = `captain-who-accents-${instanceId}`
  const lightGradientId = `captain-who-light-gradient-${instanceId}`
  const darkGradientId = `captain-who-dark-gradient-${instanceId}`

  return (
    <svg
      aria-label="Captain Who"
      className="captain-who-wordmark"
      focusable="false"
      preserveAspectRatio="xMinYMid meet"
      role="img"
      viewBox="70 155 2035 385"
    >
      <defs>
        <mask
          id={alphaMaskId}
          height={SOURCE_HEIGHT}
          maskUnits="userSpaceOnUse"
          style={{ maskType: 'alpha' }}
          width={SOURCE_WIDTH}
          x="0"
          y="0"
        >
          <image
            height={SOURCE_HEIGHT}
            href={wordmarkMaskSource}
            preserveAspectRatio="none"
            width={SOURCE_WIDTH}
            x="0"
            y="0"
          />
        </mask>

        <clipPath id={captainClipId} clipPathUnits="userSpaceOnUse">
          <rect height={SOURCE_HEIGHT} width="1202" x="0" y="0" />
        </clipPath>

        <clipPath id={whoClipId} clipPathUnits="userSpaceOnUse">
          <path clipRule="evenodd" d={WHO_WITHOUT_ACCENTS_PATH} fillRule="evenodd" />
        </clipPath>

        <clipPath id={accentClipId} clipPathUnits="userSpaceOnUse">
          <rect height="108" width="64" x="1914" y="182" />
          <rect height="94" width="84" x="1978" y="214" />
          <rect height="18" width="10" x="1968" y="290" />
          <rect height="45" width="94" x="1997" y="308" />
          <rect height="5" width="29" x="2062" y="303" />
        </clipPath>

        <linearGradient
          data-wordmark-gradient="light"
          id={lightGradientId}
          gradientUnits="userSpaceOnUse"
          x1="1227"
          x2="1701"
          y1="169"
          y2="734"
        >
          <stop offset="0%" stopColor="#061c4e" />
          <stop offset="52%" stopColor="#0b4382" />
          <stop offset="100%" stopColor="#1e79bf" />
        </linearGradient>

        <linearGradient
          data-wordmark-gradient="dark"
          id={darkGradientId}
          gradientUnits="userSpaceOnUse"
          x1="1227"
          x2="1945"
          y1="169"
          y2="567"
        >
          <stop offset="0%" stopColor="#002660" />
          <stop offset="52%" stopColor="#023d88" />
          <stop offset="100%" stopColor="#2370b6" />
        </linearGradient>
      </defs>

      <g mask={`url(#${alphaMaskId})`}>
        <rect
          className="captain-who-wordmark__captain"
          clipPath={`url(#${captainClipId})`}
          data-wordmark-layer="captain"
          height={SOURCE_HEIGHT}
          width={SOURCE_WIDTH}
          x="0"
          y="0"
        />
        <rect
          className="captain-who-wordmark__who captain-who-wordmark__who--light"
          clipPath={`url(#${whoClipId})`}
          data-wordmark-layer="who-light"
          fill={`url(#${lightGradientId})`}
          height={SOURCE_HEIGHT}
          width={SOURCE_WIDTH}
          x="0"
          y="0"
        />
        <rect
          className="captain-who-wordmark__who captain-who-wordmark__who--dark"
          clipPath={`url(#${whoClipId})`}
          data-wordmark-layer="who-dark"
          fill={`url(#${darkGradientId})`}
          height={SOURCE_HEIGHT}
          width={SOURCE_WIDTH}
          x="0"
          y="0"
        />
        <rect
          className="captain-who-wordmark__accents"
          clipPath={`url(#${accentClipId})`}
          data-wordmark-layer="accents"
          height={SOURCE_HEIGHT}
          width={SOURCE_WIDTH}
          x="0"
          y="0"
        />
      </g>
    </svg>
  )
}
