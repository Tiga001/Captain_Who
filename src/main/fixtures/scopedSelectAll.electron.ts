import { mkdirSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { app, BrowserWindow, ipcMain, Menu, WebContentsView, webContents } from 'electron'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import { installScopedSelectAllMenu, selectAllNativeForRenderer } from '../scopedSelectAllMenu'

const profile = process.argv[2]
if (!profile) throw new Error('Isolated fixture profile missing')
mkdirSync(profile, { recursive: true })
app.setPath('userData', profile)
app.setPath('sessionData', join(profile, 'session'))

async function waitFor(predicate: () => boolean | Promise<boolean>): Promise<void> {
  const deadline = Date.now() + 8_000
  while (!(await predicate())) {
    if (Date.now() > deadline) throw new Error('Selection fixture timed out')
    await new Promise((resolve) => setTimeout(resolve, 15))
  }
}

async function main(): Promise<void> {
  await app.whenReady()
  const preload = join(profile, 'preload.cjs')
  writeFileSync(
    preload,
    `const {contextBridge,ipcRenderer}=require('electron');
    contextBridge.exposeInMainWorld('probe',{
      onSelectAll:callback=>ipcRenderer.on(${JSON.stringify(HOST_CHANNELS.app.selectAllRequested)},()=>callback()),
      native:()=>ipcRenderer.invoke(${JSON.stringify(HOST_CHANNELS.app.selectAllNative)})
    });`
  )
  const window = new BrowserWindow({
    show: true,
    opacity: 0,
    skipTaskbar: true,
    width: 600,
    height: 400,
    webPreferences: { preload, contextIsolation: true, sandbox: true, nodeIntegration: false }
  })
  const original = Menu.getApplicationMenu()!
  const originalFile = original.items.find((item) => item.role?.toLowerCase() === 'filemenu')!
  const originalEdit = original.items.find((item) => item.role?.toLowerCase() === 'editmenu')!
  const originalCopy = originalEdit.submenu!.items.find(
    (item) => item.role?.toLowerCase() === 'copy'
  )!
  const originalSelect = originalEdit.submenu!.items.find(
    (item) => item.role?.toLowerCase() === 'selectall'
  )!
  ipcMain.handle(HOST_CHANNELS.app.selectAllNative, (event) =>
    selectAllNativeForRenderer(event.sender)
  )
  installScopedSelectAllMenu(() => window.webContents)
  const replacement = Menu.getApplicationMenu()!
  const edit = replacement.items.find((item) => item.role?.toLowerCase() === 'editmenu')!
  const select = edit.submenu!.items.find((item) => item.label === originalSelect.label)!
  const structure = {
    originalFileReused: replacement.items.includes(originalFile),
    originalCopyReused: edit.submenu!.items.includes(originalCopy),
    labelPreserved: select.label === originalSelect.label,
    acceleratorPreserved: select.accelerator === originalSelect.accelerator,
    onlyNativeSelectRoleRemoved: !edit.submenu!.items.some(
      (item) => item.role?.toLowerCase() === 'selectall'
    ),
    editRolePreserved: edit.role === originalEdit.role,
    topLabelsPreserved:
      replacement.items.map((item) => item.label).join('|') ===
      original.items.map((item) => item.label).join('|')
  }
  await window.loadURL(
    'data:text/html,' +
      encodeURIComponent(`
    <body><nav>SIDEBAR TEXT</nav><main id="page">PAGE ONE<p>PAGE TWO</p></main><input value="INPUT OWN VALUE">
    <script>window.requests=0;window.probe.onSelectAll(()=>{
      requests++;
      if(document.activeElement.tagName==='INPUT'){window.probe.native();return;}
      const range=document.createRange();range.selectNodeContents(document.querySelector('#page'));
      getSelection().removeAllRanges();getSelection().addRange(range);
    });</script>`)
  )
  window.focus()
  window.webContents.focus()
  await waitFor(() => webContents.getFocusedWebContents()?.id === window.webContents.id)
  const click = (): void => select.click(select, window, { triggeredByAccelerator: true })
  click()
  await waitFor(() => window.webContents.executeJavaScript('requests === 1'))
  const scoped = await window.webContents.executeJavaScript('getSelection().toString()')
  await window.webContents.executeJavaScript(
    'getSelection().removeAllRanges();document.querySelector("input").focus()'
  )
  click()
  await waitFor(() =>
    window.webContents.executeJavaScript('document.querySelector("input").selectionEnd===15')
  )
  const nativeInput = await window.webContents.executeJavaScript(
    '({requests, start:document.querySelector("input").selectionStart, end:document.querySelector("input").selectionEnd})'
  )

  const guest = new WebContentsView({
    webPreferences: { sandbox: true, contextIsolation: true, nodeIntegration: false }
  })
  window.contentView.addChildView(guest)
  guest.setBounds({ x: 300, y: 0, width: 300, height: 400 })
  await guest.webContents.loadURL('data:text/html,<body>GUEST ONE<p>GUEST TWO</p></body>')
  guest.webContents.focus()
  await waitFor(() => webContents.getFocusedWebContents()?.id === guest.webContents.id)
  click()
  await waitFor(() =>
    guest.webContents.executeJavaScript('getSelection().toString().includes("GUEST TWO")')
  )
  const guestSelection = await guest.webContents.executeJavaScript('getSelection().toString()')
  const hostRequestsAfterGuest = await window.webContents.executeJavaScript('requests')
  await window.webContents.executeJavaScript(
    'document.querySelector("input").setSelectionRange(2, 3)'
  )
  selectAllNativeForRenderer(window.webContents)
  const unfocusedFallbackIgnored = await window.webContents.executeJavaScript(
    'document.querySelector("input").selectionStart===2&&document.querySelector("input").selectionEnd===3'
  )
  console.log(
    'MYCOPILOT_SCOPED_SELECT_ALL=' +
      JSON.stringify({
        structure,
        scoped,
        nativeInput,
        guestSelection,
        hostRequestsAfterGuest,
        unfocusedFallbackIgnored,
        isolatedProfile: app.getPath('userData') === profile
      })
  )
  guest.webContents.close()
  window.destroy()
  app.quit()
}

void main().catch((error) => {
  console.error(error)
  app.exit(1)
})
