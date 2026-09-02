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
    const captain = logo.querySelector<SVGRectElement>('[data-wordmark-layer="captain"]')!
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
    ).toEqual(['#061c4e', '#0b4382', '#1e79bf'])
    expect(
      [...darkGradient.querySelectorAll('stop')].map((stop) => stop.getAttribute('stop-color'))
    ).toEqual(['#002660', '#023d88', '#2370b6'])
    expect(getComputedStyle(captain).fill).toBe('rgb(11, 28, 54)')
    expect(getComputedStyle(lightWho).display).not.toBe('none')
    expect(getComputedStyle(darkWho).display).toBe('none')
    expect(getComputedStyle(accents).fill).toBe('rgb(217, 70, 239)')

    root.dataset.colorScheme = 'dark'
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))

    const darkBounds = logo.getBoundingClientRect()
    expect(getComputedStyle(captain).fill).toBe('rgb(254, 254, 254)')
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
    const captain = logo.querySelector<SVGRectElement>('[data-wordmark-layer="captain"]')!
    const accents = logo.querySelector<SVGRectElement>('[data-wordmark-layer="accents"]')!
    const captainColor = getComputedStyle(captain).fill

    expect(getComputedStyle(accents).fill).toBe('rgb(37, 99, 235)')

    root.style.setProperty('--mc-color-text-accent', '#f97316')
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))

    expect(getComputedStyle(accents).fill).toBe('rgb(249, 115, 22)')
    expect(getComputedStyle(captain).fill).toBe(captainColor)
  })
})
