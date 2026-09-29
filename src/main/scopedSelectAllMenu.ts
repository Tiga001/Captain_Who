import { Menu, MenuItem, webContents, type WebContents } from 'electron'
import { HOST_CHANNELS } from '@mycopilot/host-api'

/** Keep native editors and guest pages on Chromium's own selection implementation. */
export function selectAllNativeForRenderer(sender: WebContents): void {
  const focused = webContents.getFocusedWebContents()
  if (!sender.isDestroyed() && focused?.id === sender.id) sender.selectAll()
}

/** The macOS Edit menu invokes a native selector without dispatching a DOM keydown. */
export function installScopedSelectAllMenu(getMainContents: () => WebContents | null): void {
  const existing = Menu.getApplicationMenu()
  if (!existing) return
  const selectAll = (): void => {
    const focused = webContents.getFocusedWebContents()
    if (!focused || focused.isDestroyed()) return
    const main = getMainContents()
    if (main && !main.isDestroyed() && focused.id === main.id) {
      focused.send(HOST_CHANNELS.app.selectAllRequested)
    } else {
      focused.selectAll()
    }
  }
  const replace = (menu: Menu): Menu | null => {
    let changed = false
    const items = menu.items.map((item) => {
      if (item.role?.toLowerCase() === 'selectall') {
        changed = true
        return new MenuItem({
          id: item.id,
          label: item.label,
          enabled: item.enabled,
          visible: item.visible,
          accelerator: item.accelerator ?? 'CommandOrControl+A',
          click: selectAll
        })
      }
      const submenu = item.submenu && replace(item.submenu)
      if (!submenu) return item
      changed = true
      return new MenuItem({
        id: item.id,
        label: item.label,
        role: item.role,
        enabled: item.enabled,
        visible: item.visible,
        submenu
      })
    })
    // Electron does not support removing items from its live application menu. Reuse every
    // unchanged MenuItem; only the select-all leaf and its menu containers are replaced.
    return changed ? Menu.buildFromTemplate(items) : null
  }
  const updated = replace(existing)
  if (updated) Menu.setApplicationMenu(updated)
}
