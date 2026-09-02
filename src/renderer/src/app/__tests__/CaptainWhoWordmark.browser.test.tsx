import { afterEach, describe, expect, it } from 'vitest'
import { render } from 'vitest-browser-react'
import { CaptainWhoWordmark } from '../shell/sidebar/CaptainWhoWordmark'

const root = document.documentElement
const originalColorScheme = root.dataset.colorScheme
const originalAccent = root.style.getPropertyValue('--mc-color-text-accent')

afterEach(() => {
  if (originalColorScheme) root.dataset.colorScheme = originalColorScheme
  else delete root.dataset.colorScheme

  if (originalAccent) root.style.setProperty('--mc-color-text-accent', originalAccent)
  else root.style.removeProperty('--mc-color-text-accent')
})

describe('CaptainWhoWordmark', () => {
  it('switches only presentation colors while keeping the original geometry fixed', async () => {
    root.dataset.colorScheme = 'light'
    root.style.setProperty('--mc-color-text-accent', '#d946ef')

    const screen = await render(
      <div style={{ width: 140 }}>
        <CaptainWhoWordmark />
      </div>
    )
    const logo = screen.getByRole('img', { name: 'Captain Who' }).element() as SVGSVGElement
    const lightCaptain = logo.querySelector<SVGRectElement>(
      '[data-wordmark-layer="captain-light"]'
    )!
    const darkCaptain = logo.querySelector<SVGRectElement>('[data-wordmark-layer="captain-dark"]')!
    const lightWho = logo.querySelector<SVGRectElement>('[data-wordmark-layer="who-light"]')!
    const darkWho = logo.querySelector<SVGRectElement>('[data-wordmark-layer="who-dark"]')!
    const accents = logo.querySelector<SVGRectElement>('[data-wordmark-layer="accents"]')!
    const lightGradient = logo.querySelector<SVGLinearGradientElement>(
      '[data-wordmark-gradient="light"]'
    )!
    const darkGradient = logo.querySelector<SVGLinearGradientElement>(
      '[data-wordmark-gradient="dark"]'
    )!
    const initialBounds = logo.getBoundingClientRect()

    expect(logo.getAttribute('viewBox')).toBe('70 155 2035 385')
    expect(
      [...lightGradient.querySelectorAll('stop')].map((stop) => stop.getAttribute('stop-color'))
    ).toEqual(['#173a5e', '#1e527b', '#256f9f'])
    expect(
      [...darkGradient.querySelectorAll('stop')].map((stop) => stop.getAttribute('stop-color'))
    ).toEqual(['#f4f7fb', '#a9cde8', '#4a94cd'])
    expect(lightCaptain.getAttribute('fill')).toBe('#2b3540')
    expect(darkCaptain.getAttribute('fill')).toBe('#fefefe')
    expect(lightWho.getAttribute('fill')).toContain('captain-who-light-gradient-')
    expect(darkWho.getAttribute('fill')).toContain('captain-who-dark-gradient-')
    expect(getComputedStyle(lightCaptain).display).not.toBe('none')
    expect(getComputedStyle(darkCaptain).display).toBe('none')
    expect(getComputedStyle(lightWho).display).not.toBe('none')
    expect(getComputedStyle(darkWho).display).toBe('none')
    expect(getComputedStyle(accents).fill).toBe('rgb(217, 70, 239)')

    const maskImage = logo.querySelector('mask image')
    const accentClip = logo.querySelector<SVGClipPathElement>(
      'clipPath[id^="captain-who-accents-"]'
    )
    const accentRects = [...(accentClip?.querySelectorAll('rect') ?? [])].map((rect) => [
      rect.getAttribute('x'),
      rect.getAttribute('y'),
      rect.getAttribute('width'),
      rect.getAttribute('height')
    ])
    expect(maskImage?.getAttribute('width')).toBe('2172')
    expect(maskImage?.getAttribute('height')).toBe('724')
    expect(accentRects).toEqual([
      ['1914', '182', '64', '108'],
      ['1978', '214', '84', '94'],
      ['1968', '290', '10', '18'],
      ['1997', '308', '94', '45'],
      ['2062', '303', '29', '5']
    ])

    root.dataset.colorScheme = 'dark'
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))

    const darkBounds = logo.getBoundingClientRect()
    expect(getComputedStyle(lightCaptain).display).toBe('none')
    expect(getComputedStyle(darkCaptain).display).not.toBe('none')
    expect(getComputedStyle(lightWho).display).toBe('none')
    expect(getComputedStyle(darkWho).display).not.toBe('none')
    expect(darkBounds.width).toBeCloseTo(initialBounds.width, 2)
    expect(darkBounds.height).toBeCloseTo(initialBounds.height, 2)
  })

  it('binds the three isolated accents to the live theme accent token', async () => {
    root.dataset.colorScheme = 'light'
    root.style.setProperty('--mc-color-text-accent', '#2563eb')

    const screen = await render(
      <div style={{ width: 140 }}>
        <CaptainWhoWordmark />
      </div>
    )
    const logo = screen.getByRole('img', { name: 'Captain Who' }).element() as SVGSVGElement
    const captain = logo.querySelector<SVGRectElement>('[data-wordmark-layer="captain-light"]')!
    const accents = logo.querySelector<SVGRectElement>('[data-wordmark-layer="accents"]')!
    const captainColor = getComputedStyle(captain).fill

    expect(getComputedStyle(accents).fill).toBe('rgb(37, 99, 235)')

    root.style.setProperty('--mc-color-text-accent', '#f97316')
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))

    expect(getComputedStyle(accents).fill).toBe('rgb(249, 115, 22)')
    expect(getComputedStyle(captain).fill).toBe(captainColor)
  })
})
