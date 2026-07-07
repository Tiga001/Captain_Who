// Electron main client.
import { randomUUID } from 'node:crypto'
import { BrowserWindow, WebContentsView, type Rectangle } from 'electron'

export interface BrowserCreateInput {
  id?: string
  bounds?: Rectangle
}

export interface BrowserNavigateInput {
  id: string
  url: string
}

export interface BrowserSetBoundsInput {
  id: string
  bounds: Rectangle
}

export interface BrowserDestroyInput {
  id: string
}

export class BrowserWebContentsViewManager {
  private readonly views = new Map<string, WebContentsView>()

  constructor(private readonly ownerWindow: BrowserWindow) {}

  async create(input: BrowserCreateInput = {}): Promise<{ id: string }> {
    const id = input.id ?? randomUUID()
    if (this.views.has(id)) {
      return { id }
    }

    const view = new WebContentsView()
    this.ownerWindow.contentView.addChildView(view)

    if (input.bounds) {
      view.setBounds(input.bounds)
    }

    this.views.set(id, view)
    return { id }
  }

  async navigate(input: BrowserNavigateInput): Promise<{ id: string; url: string }> {
    const view = this.requireView(input.id)
    await view.webContents.loadURL(input.url)
    return input
  }

  async setBounds(input: BrowserSetBoundsInput): Promise<{ id: string }> {
    this.requireView(input.id).setBounds(input.bounds)
    return { id: input.id }
  }

  async destroy(input: BrowserDestroyInput): Promise<{ id: string }> {
    const view = this.requireView(input.id)
    this.ownerWindow.contentView.removeChildView(view)
    view.webContents.close()
    this.views.delete(input.id)
    return { id: input.id }
  }

  destroyAll(): void {
    for (const id of this.views.keys()) {
      void this.destroy({ id })
    }
  }

  private requireView(id: string): WebContentsView {
    const view = this.views.get(id)
    if (!view) {
      throw new Error(`Browser view not found: ${id}`)
    }

    return view
  }
}
