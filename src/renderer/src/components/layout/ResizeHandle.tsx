import { useEffect, useRef, useState } from 'react'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'

interface ResizeHandleProps {
  side: 'left' | 'right'
  onResize: (deltaX: number) => void
}

export function ResizeHandle({ side, onResize }: ResizeHandleProps) {
  const { t } = useFrontendConfig()
  const startX = useRef(0)
  const [dragging, setDragging] = useState(false)

  useEffect(() => {
    if (!dragging) return

    const handlePointerMove = (event: PointerEvent) => {
      const deltaX = event.clientX - startX.current
      startX.current = event.clientX
      onResize(deltaX)
    }

    const stopDragging = () => setDragging(false)

    document.body.classList.add('is-resizing')
    window.addEventListener('pointermove', handlePointerMove)
    window.addEventListener('pointerup', stopDragging, { once: true })
    window.addEventListener('pointercancel', stopDragging, { once: true })

    return () => {
      document.body.classList.remove('is-resizing')
      window.removeEventListener('pointermove', handlePointerMove)
      window.removeEventListener('pointerup', stopDragging)
      window.removeEventListener('pointercancel', stopDragging)
    }
  }, [dragging, onResize])

  return (
    <div
      className={`resize-handle resize-handle--${side}`}
      role="separator"
      aria-label={side === 'left' ? t('app.resizeLeftSidebar') : t('app.resizeRightSidebar')}
      aria-orientation="vertical"
      onPointerDown={(event) => {
        event.preventDefault()
        startX.current = event.clientX
        setDragging(true)
      }}
    />
  )
}
