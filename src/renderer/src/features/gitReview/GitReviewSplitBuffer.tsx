import type { CSSProperties, ReactNode } from 'react'

interface GitReviewSplitBufferProps {
  lineCount: number
}

type BufferStyle = CSSProperties & { '--git-review-buffer-lines': number }

/** One continuous missing-side region; the stripe phase intentionally spans every omitted row. */
export function GitReviewSplitBuffer({ lineCount }: GitReviewSplitBufferProps): ReactNode {
  const bufferLines = Math.max(1, lineCount)
  const style: BufferStyle = { '--git-review-buffer-lines': bufferLines }
  return (
    <div
      className="git-review__split-buffer"
      data-buffer-size={bufferLines}
      data-content-buffer="true"
      style={style}
    >
      <span className="git-review__split-buffer-gutter" aria-hidden="true" />
      <span className="git-review__split-buffer-content" aria-hidden="true" />
    </div>
  )
}
