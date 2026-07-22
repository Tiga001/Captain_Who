import { app, BrowserWindow, nativeImage, nativeTheme, type NativeImage } from 'electron'
import darkIconPath from '../../resources/icon-dark.png?asset'
import darkMacIconPath from '../../resources/icon-dark-macos.png?asset'
import lightIconPath from '../../resources/icon-light.png?asset'
import lightMacIconPath from '../../resources/icon-light-macos.png?asset'

type AppIconVariant = 'light' | 'dark'

const iconPaths: Record<AppIconVariant, string> =
  process.platform === 'darwin'
    ? { light: lightMacIconPath, dark: darkMacIconPath }
    : { light: lightIconPath, dark: darkIconPath }
const iconCache = new Map<AppIconVariant, NativeImage>()

function getAppIconVariant(): AppIconVariant {
  return nativeTheme.shouldUseDarkColors ? 'dark' : 'light'
}

function getAppIcon(variant: AppIconVariant): NativeImage {
  const cachedIcon = iconCache.get(variant)
  if (cachedIcon) return cachedIcon

  const icon = nativeImage.createFromPath(iconPaths[variant])
  if (icon.isEmpty()) {
    throw new Error(`Failed to load ${variant} app icon from ${iconPaths[variant]}`)
  }

  iconCache.set(variant, icon)
  return icon
}

export function getAdaptiveAppIcon(): NativeImage {
  return getAppIcon(getAppIconVariant())
}

function applyAdaptiveAppIcon(): void {
  const icon = getAdaptiveAppIcon()

  if (process.platform === 'darwin') {
    app.dock?.setIcon(icon)
    return
  }

  for (const window of BrowserWindow.getAllWindows()) {
    if (!window.isDestroyed()) {
      window.setIcon(icon)
    }
  }
}

export function installAdaptiveAppIcon(): () => void {
  const handleThemeUpdated = (): void => applyAdaptiveAppIcon()

  nativeTheme.on('updated', handleThemeUpdated)
  applyAdaptiveAppIcon()

  return () => nativeTheme.off('updated', handleThemeUpdated)
}
