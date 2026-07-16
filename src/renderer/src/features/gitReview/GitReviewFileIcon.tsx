import type { ReactNode } from 'react'
import { FileTypeIcon } from '../../components/files/FileTypeIcon'

interface GitReviewFileIconProps {
  path: string
}

export function GitReviewFileIcon({ path }: GitReviewFileIconProps): ReactNode {
  return <FileTypeIcon className="git-review__file-icon" path={path} />
}
