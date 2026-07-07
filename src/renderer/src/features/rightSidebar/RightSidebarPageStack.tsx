// Renderer UI.
// Implements the right-sidebar generic keep-alive page stack.
// Switching pages only changes visibility; mounted module components stay alive until their page closes.

import type { ReactNode } from 'react'
import type { RightSidebarPage } from './rightSidebarTypes'

interface RightSidebarPageStackProps {
  activePageId: string | null
  pages: RightSidebarPage[]
  renderPage: (page: RightSidebarPage, isActive: boolean) => ReactNode
}

export function RightSidebarPageStack({
  activePageId,
  pages,
  renderPage
}: RightSidebarPageStackProps) {
  return (
    <div className="right-sidebar__page-stack">
      {pages.map((page) => {
        const isActive = page.id === activePageId

        return (
          <section
            className="right-sidebar__page"
            data-active={isActive ? 'true' : undefined}
            aria-hidden={isActive ? undefined : true}
            key={page.id}
          >
            {renderPage(page, isActive)}
          </section>
        )
      })}
    </div>
  )
}
