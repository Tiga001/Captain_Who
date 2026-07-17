import { describe, expect, it } from 'vitest'
import {
  GitReviewDiffRequestQueue,
  type GitReviewDiffPriority,
  type GitReviewDiffQueueItem
} from '../gitReviewDiffRequestQueue'

function item(fileId: string, priority: GitReviewDiffPriority): GitReviewDiffQueueItem {
  return {
    fileId,
    priority,
    requestKey: `request:${fileId}`,
    snapshotId: 'snapshot-1'
  }
}

describe('GitReviewDiffRequestQueue', () => {
  it('serves selected-file work before viewport prefetch work', () => {
    const queue = new GitReviewDiffRequestQueue(4)
    queue.enqueue(item('near-1', 'normal'))
    queue.enqueue(item('selected', 'high'))
    queue.enqueue(item('near-2', 'normal'))

    expect(queue.shift()?.fileId).toBe('selected')
    expect(queue.shift()?.fileId).toBe('near-1')
    expect(queue.shift()?.fileId).toBe('near-2')
  })

  it('bounds pending work and lets a selected file evict the newest normal prefetch', () => {
    const queue = new GitReviewDiffRequestQueue(2)
    queue.enqueue(item('near-1', 'normal'))
    queue.enqueue(item('near-2', 'normal'))

    expect(queue.enqueue(item('near-3', 'normal'))).toEqual({ accepted: false })
    expect(queue.enqueue(item('selected', 'high'))).toEqual({
      accepted: true,
      evicted: item('near-2', 'normal')
    })
    expect(queue.size).toBe(2)
    expect(queue.shift()?.fileId).toBe('selected')
    expect(queue.shift()?.fileId).toBe('near-1')
  })

  it('promotes existing work without duplicating it', () => {
    const queue = new GitReviewDiffRequestQueue(3)
    const near = item('near', 'normal')
    queue.enqueue(near)
    queue.enqueue(item('other', 'normal'))

    expect(queue.promote(near.fileId, near.requestKey)).toBe(true)
    expect(queue.size).toBe(2)
    expect(queue.shift()?.fileId).toBe('near')
    expect(queue.shift()?.fileId).toBe('other')
  })

  it('removes stale viewport demand without disturbing a replacement request', () => {
    const queue = new GitReviewDiffRequestQueue(3)
    const stale = item('stale', 'normal')
    queue.enqueue(stale)

    expect(queue.remove(stale.fileId, 'another-request')).toBeUndefined()
    expect(queue.remove(stale.fileId, stale.requestKey)).toEqual(stale)
    expect(queue.size).toBe(0)
  })
})
