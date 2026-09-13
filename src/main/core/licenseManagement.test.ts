import { beforeEach, afterEach, expect, it, vi } from 'vitest'
import type { AuthState } from '@mycopilot/host-api'
import { LicenseManagementService } from '../auth/LicenseManagementService'
import { ACCOUNT_PAGES } from '../auth/accountConfig'

function setup() {
  let state = { status: 'signedIn', profile: { userId: 'a' } } as AuthState
  const open = vi.fn<(url: string) => Promise<void>>().mockResolvedValue(undefined)
  const refresh = vi.fn().mockResolvedValue({ status: 'allowed' })
  const service = new LicenseManagementService(
    { getState: () => state },
    { refreshAfterManagement: refresh },
    open
  )
  return {
    service,
    open,
    refresh,
    account: (userId: string | null) => {
      state = {
        status: userId ? 'signedIn' : 'signedOut',
        profile: userId ? { userId } : null
      } as AuthState
    }
  }
}
beforeEach(() => {
  vi.useFakeTimers()
  vi.setSystemTime(new Date('2026-09-13T08:00:00Z'))
})
afterEach(() => vi.useRealTimers())

it('only explicitly opens the allowlisted website and coalesces rapid clicks', async () => {
  const s = setup()
  s.service.onBlur()
  s.service.onFocus()
  expect(s.open).not.toHaveBeenCalled()
  expect(s.refresh).not.toHaveBeenCalled()
  await Promise.all([s.service.open(), s.service.open()])
  await s.service.open()
  expect(s.open).toHaveBeenCalledExactlyOnceWith(ACCOUNT_PAGES.profile)
  s.service.onFocus()
  expect(s.refresh).not.toHaveBeenCalled()
  s.service.onBlur()
  s.service.onFocus()
  s.service.onFocus()
  expect(s.refresh).toHaveBeenCalledOnce()
})

it('does not revalidate on unrelated focus, account changes, or expired management visits', async () => {
  const s = setup()
  await s.service.open()
  s.service.onBlur()
  s.account('b')
  s.service.onFocus()
  expect(s.refresh).not.toHaveBeenCalled()
  await vi.advanceTimersByTimeAsync(10_000)
  await s.service.open()
  s.service.onBlur()
  await vi.advanceTimersByTimeAsync(10 * 60_000 + 1)
  s.service.onFocus()
  expect(s.refresh).not.toHaveBeenCalled()
  s.account(null)
  await expect(s.service.open()).rejects.toThrow('ACCOUNT_LOGIN_REQUIRED')
})

it('allows a retry after an OS open failure, but rate-limits successive return checks', async () => {
  const s = setup()
  s.open.mockRejectedValueOnce(new Error('OS failure'))
  await expect(s.service.open()).rejects.toThrow('Unable to open')
  await s.service.open()
  s.service.onBlur()
  s.service.onFocus()
  expect(s.refresh).toHaveBeenCalledOnce()
  await vi.advanceTimersByTimeAsync(10_000)
  await s.service.open()
  s.service.onBlur()
  s.service.onFocus()
  expect(s.refresh).toHaveBeenCalledOnce()
})

it('does not refresh after disposal', async () => {
  const s = setup()
  await s.service.open()
  s.service.onBlur()
  s.service.dispose()
  s.service.onFocus()
  expect(s.refresh).not.toHaveBeenCalled()
})
