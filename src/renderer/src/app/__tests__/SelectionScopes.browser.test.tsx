import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import {
  installSelectionScopes,
  requestScopedSelectAll,
  SELECTION_ALL_EVENT,
  SELECTION_COPY_EVENT,
  type SelectionCopyAllEvent
} from '../../components/selection/selectionScope'

let fixture: HTMLDivElement
let release: () => void

beforeEach(() => {
  fixture = document.createElement('div')
  fixture.innerHTML = `
    <nav><button id="left">Left navigation</button></nav>
    <main data-selection-region="main">
      <header>Main toolbar</header>
      <section data-selection-region="conversation">
        <div id="main" data-selection-content="primary">Main conversation</div>
        <textarea id="composer">Draft text</textarea>
      </section>
    </main>
    <aside data-selection-region="sidebar">
      <header><button id="tab">File tab</button></header>
      <section data-selection-content="page">
        <header>Breadcrumb</header>
        <nav><input id="search" value="Search tree" /></nav>
        <article id="file" data-selection-content="primary">File body</article>
      </section>
      <section hidden data-selection-content="page">
        <article data-selection-content="primary">Hidden retained page</article>
      </section>
    </aside>`
  document.body.append(fixture)
  release = installSelectionScopes()
})

afterEach(() => {
  release()
  document.getSelection()?.removeAllRanges()
  fixture.remove()
})

function element(id: string): HTMLElement {
  return fixture.querySelector<HTMLElement>(`#${id}`)!
}

function pointAt(id: string) {
  element(id).dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }))
}

function selectAll(control = false): KeyboardEvent {
  const event = new KeyboardEvent('keydown', {
    key: 'a',
    metaKey: !control,
    ctrlKey: control,
    bubbles: true,
    cancelable: true
  })
  ;(document.activeElement ?? document.body).dispatchEvent(event)
  return event
}

it('selects only the active sidebar body from both its content and its tab', () => {
  for (const id of ['file', 'tab']) {
    pointAt(id)
    expect(selectAll().defaultPrevented).toBe(true)
    expect(document.getSelection()?.toString()).toBe('File body')
  }
})

it('selects only the main conversation after moving outside the sidebar', () => {
  pointAt('file')
  selectAll()
  pointAt('main')
  expect(selectAll(true).defaultPrevented).toBe(true)
  expect(document.getSelection()?.toString()).toBe('Main conversation')
  element('left').focus()
  selectAll()
  expect(document.getSelection()?.toString()).toBe('Main conversation')
})

it('preserves input select-all, then respects a static page click even if input focus is retained', () => {
  element('composer').focus()
  expect(selectAll().defaultPrevented).toBe(false)
  pointAt('file')
  expect(selectAll().defaultPrevented).toBe(true)
  expect(document.getSelection()?.toString()).toBe('File body')
  element('search').focus()
  expect(selectAll().defaultPrevented).toBe(false)
})

it('routes native menu requests to the same region and delegates native controls', () => {
  pointAt('file')
  expect(requestScopedSelectAll()).toBe(true)
  expect(document.getSelection()?.toString()).toBe('File body')
  element('composer').focus()
  expect(requestScopedSelectAll()).toBe(false)
  const terminal = document.createElement('div')
  terminal.setAttribute('data-selection-native', 'true')
  terminal.innerHTML = '<textarea></textarea>'
  fixture.append(terminal)
  const nativeAll = vi.fn((event: Event) => event.preventDefault())
  terminal.addEventListener(SELECTION_ALL_EVENT, nativeAll)
  terminal.querySelector('textarea')!.focus()
  expect(requestScopedSelectAll()).toBe(true)
  expect(nativeAll).toHaveBeenCalledOnce()
  expect(selectAll(true).defaultPrevented).toBe(false)
})

it('does not let hidden or inert retained pages become selection targets', () => {
  pointAt('file')
  fixture.querySelector('aside')!.setAttribute('inert', '')
  selectAll()
  expect(document.getSelection()?.toString()).toBe('Main conversation')
  fixture.querySelector('aside')!.removeAttribute('inert')
  fixture.querySelector('main')!.setAttribute('inert', '')
  pointAt('tab')
  selectAll()
  expect(document.getSelection()?.toString()).toBe('File body')
})

it('ignores a collapsed sidebar even when its retained page overrides CSS visibility', () => {
  pointAt('file')
  const sidebar = fixture.querySelector('aside')!
  sidebar.style.visibility = 'hidden'
  element('file').style.visibility = 'visible'
  sidebar.dataset.selectionHidden = 'true'
  selectAll()
  expect(document.getSelection()?.toString()).toBe('Main conversation')
})

it('retains sidebar ownership for menus rendered in a portal', () => {
  const menu = document.createElement('div')
  menu.dataset.selectionOwner = 'sidebar'
  menu.innerHTML = '<button role="menuitem">Open files</button>'
  fixture.append(menu)
  menu.querySelector('button')!.focus()
  selectAll()
  expect(document.getSelection()?.toString()).toBe('File body')
})

it('keeps modal confirmation selection inside the alert dialog', () => {
  const dialog = document.createElement('div')
  dialog.setAttribute('role', 'alertdialog')
  dialog.setAttribute('aria-modal', 'true')
  dialog.innerHTML = '<p>Confirm operation</p><button>Cancel</button>'
  fixture.append(dialog)
  dialog.querySelector('button')!.focus()
  requestScopedSelectAll()
  expect(document.getSelection()?.toString()).toContain('Confirm operation')
  expect(document.getSelection()?.toString()).not.toContain('Main conversation')
  expect(document.getSelection()?.toString()).not.toContain('File body')
})

it('selects another independent page without including the main chat or sidebar', () => {
  const panel = document.createElement('section')
  panel.dataset.selectionRegion = 'bottom'
  panel.innerHTML = '<button>Tab</button><div data-selection-content="page">Review body</div>'
  fixture.append(panel)
  panel.querySelector('button')!.focus()
  selectAll()
  expect(document.getSelection()?.toString()).toBe('Review body')
})

it('supports semantic full-copy without changing later partial copies', () => {
  element('file').addEventListener(SELECTION_COPY_EVENT, ((event: SelectionCopyAllEvent) => {
    event.detail.clipboardData.setData('text/plain', 'Original\r\nsource\t')
    event.preventDefault()
  }) as EventListener)
  pointAt('file')
  selectAll()
  const copy = () => {
    const event = new ClipboardEvent('copy', {
      bubbles: true,
      cancelable: true,
      clipboardData: new DataTransfer()
    })
    document.body.dispatchEvent(event)
    return event
  }
  const full = copy()
  expect(full.defaultPrevented).toBe(true)
  expect(full.clipboardData?.getData('text/plain')).toBe('Original\r\nsource\t')
  const range = document.createRange()
  range.setStart(element('file').firstChild!, 0)
  range.setEnd(element('file').firstChild!, 4)
  document.getSelection()?.removeAllRanges()
  document.getSelection()?.addRange(range)
  expect(copy().defaultPrevented).toBe(false)
})

it('keeps one controller with multiple consumers and releases listeners once all leave', () => {
  const secondRelease = installSelectionScopes()
  release()
  const request = vi.fn()
  element('file').addEventListener(SELECTION_ALL_EVENT, request)
  pointAt('file')
  selectAll()
  expect(request).toHaveBeenCalledOnce()
  secondRelease()
  secondRelease()
  expect(selectAll().defaultPrevented).toBe(false)
})
