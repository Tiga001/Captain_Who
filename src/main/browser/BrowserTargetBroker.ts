import type { Session, WebContents } from 'electron'
import { ElectronGuestCdpTransport } from './ElectronGuestCdpTransport'

const MAX_SURFACE_ID_LENGTH = 256

export type BrowserTargetBrokerErrorCode =
  | 'duplicate_guest'
  | 'guest_not_found'
  | 'invalid_guest'
  | 'invalid_partition'
  | 'surface_conflict'
  | 'target_busy'
  | 'target_closed'

export class BrowserTargetBrokerError extends Error {
  readonly name = 'BrowserTargetBrokerError'

  constructor(readonly code: BrowserTargetBrokerErrorCode) {
    super(code)
  }
}

export interface ManagedBrowserGuestRegistration {
  guest: WebContents
  host: WebContents
  partition: string
}

export interface ManagedBrowserSurfaceClaim {
  guestWebContentsId: number
  host: WebContents
  surfaceId: string
}

interface RegisteredGuest {
  activeTransport?: ElectronGuestCdpTransport
  connecting: boolean
  guest: WebContents
  handleGuestDestroyed: () => void
  handleHostDestroyed: () => void
  host: WebContents
  partition: string
  pendingTransport?: ElectronGuestCdpTransport
  surfaceId?: string
}

export class BrowserTargetBroker {
  private readonly guests = new Map<number, RegisteredGuest>()
  private readonly surfaces = new Map<string, number>()
  private disposed = false

  constructor(
    private readonly expectedPartition: string,
    private readonly expectedSession: Session
  ) {}

  registerManagedGuest(input: ManagedBrowserGuestRegistration): void {
    this.assertUsable()
    this.assertRegistration(input)

    const existing = this.guests.get(input.guest.id)
    if (existing) {
      if (existing.guest === input.guest && existing.host === input.host) return
      throw new BrowserTargetBrokerError('duplicate_guest')
    }

    const handleGuestDestroyed = (): void =>
      this.unregisterGuest(input.guest.id, input.guest, false)
    const handleHostDestroyed = (): void => this.unregisterGuest(input.guest.id, input.guest, true)
    const record: RegisteredGuest = {
      ...input,
      connecting: false,
      handleGuestDestroyed,
      handleHostDestroyed
    }
    this.guests.set(input.guest.id, record)
    input.guest.once('destroyed', handleGuestDestroyed)
    input.host.once('destroyed', handleHostDestroyed)
  }

  claimSurface(input: ManagedBrowserSurfaceClaim): void {
    this.assertUsable()
    if (!isValidSurfaceId(input.surfaceId)) {
      throw new BrowserTargetBrokerError('surface_conflict')
    }
    const record = this.guests.get(input.guestWebContentsId)
    if (!record || record.host !== input.host) {
      throw new BrowserTargetBrokerError('guest_not_found')
    }
    this.revalidateRecord(record)

    const claimedGuestId = this.surfaces.get(input.surfaceId)
    if (claimedGuestId !== undefined && claimedGuestId !== input.guestWebContentsId) {
      throw new BrowserTargetBrokerError('surface_conflict')
    }
    if (record.surfaceId !== undefined && record.surfaceId !== input.surfaceId) {
      throw new BrowserTargetBrokerError('surface_conflict')
    }

    record.surfaceId = input.surfaceId
    this.surfaces.set(input.surfaceId, input.guestWebContentsId)
  }

  async connect(surfaceId: string): Promise<ElectronGuestCdpTransport> {
    this.assertUsable()
    const record = this.getClaimedGuest(surfaceId)
    this.revalidateRecord(record)
    if (record.connecting || record.activeTransport) {
      throw new BrowserTargetBrokerError('target_busy')
    }
    if (record.guest.isDestroyed()) throw new BrowserTargetBrokerError('target_closed')

    record.connecting = true
    try {
      const transport = new ElectronGuestCdpTransport(record.guest, (closedTransport) => {
        if (record.activeTransport === closedTransport) record.activeTransport = undefined
        if (record.pendingTransport === closedTransport) record.pendingTransport = undefined
      })
      record.pendingTransport = transport
      await transport.attach()
      if (
        record.guest.isDestroyed() ||
        record.host.isDestroyed() ||
        this.disposed ||
        !this.isExactGuest(record) ||
        this.guests.get(record.guest.id) !== record ||
        record.surfaceId !== surfaceId ||
        this.surfaces.get(surfaceId) !== record.guest.id
      ) {
        transport.close()
        throw new BrowserTargetBrokerError('target_closed')
      }
      record.pendingTransport = undefined
      record.activeTransport = transport
      return transport
    } catch (error) {
      if (error instanceof BrowserTargetBrokerError) throw error
      throw new BrowserTargetBrokerError('target_closed')
    } finally {
      record.connecting = false
    }
  }

  releaseSurface(surfaceId: string): void {
    const guestId = this.surfaces.get(surfaceId)
    if (guestId === undefined) return
    const record = this.guests.get(guestId)
    record?.pendingTransport?.close()
    record?.activeTransport?.close()
    if (record?.surfaceId === surfaceId) record.surfaceId = undefined
    this.surfaces.delete(surfaceId)
  }

  dispose(): void {
    if (this.disposed) return
    this.disposed = true
    for (const record of this.guests.values()) {
      this.removeRecordListeners(record)
      record.pendingTransport?.close()
      record.activeTransport?.close()
    }
    this.guests.clear()
    this.surfaces.clear()
  }

  snapshot(): { activeConnections: number; registeredGuests: number; claimedSurfaces: number } {
    let activeConnections = 0
    for (const record of this.guests.values()) {
      if (record.activeTransport) activeConnections += 1
    }
    return {
      activeConnections,
      registeredGuests: this.guests.size,
      claimedSurfaces: this.surfaces.size
    }
  }

  private getClaimedGuest(surfaceId: string): RegisteredGuest {
    const guestId = this.surfaces.get(surfaceId)
    const record = guestId === undefined ? undefined : this.guests.get(guestId)
    if (!record || record.surfaceId !== surfaceId) {
      throw new BrowserTargetBrokerError('guest_not_found')
    }
    return record
  }

  private unregisterGuest(guestId: number, guest: WebContents, closeTransports: boolean): void {
    const record = this.guests.get(guestId)
    if (!record || record.guest !== guest) return
    this.removeRecordListeners(record)
    if (closeTransports) {
      record.pendingTransport?.close()
      record.activeTransport?.close()
    }
    if (record.surfaceId) this.surfaces.delete(record.surfaceId)
    this.guests.delete(guestId)
  }

  private assertRegistration(input: ManagedBrowserGuestRegistration): void {
    if (
      input.partition !== this.expectedPartition ||
      input.guest.session !== this.expectedSession
    ) {
      throw new BrowserTargetBrokerError('invalid_partition')
    }
    if (
      input.guest.getType() !== 'webview' ||
      input.guest.hostWebContents !== input.host ||
      input.guest.isDestroyed() ||
      input.host.isDestroyed()
    ) {
      throw new BrowserTargetBrokerError('invalid_guest')
    }
  }

  private revalidateRecord(record: RegisteredGuest): void {
    if (
      record.partition !== this.expectedPartition ||
      record.guest.session !== this.expectedSession
    ) {
      this.unregisterGuest(record.guest.id, record.guest, true)
      throw new BrowserTargetBrokerError('invalid_partition')
    }
    if (!this.isExactGuest(record)) {
      this.unregisterGuest(record.guest.id, record.guest, true)
      throw new BrowserTargetBrokerError(
        record.guest.isDestroyed() || record.host.isDestroyed() ? 'target_closed' : 'invalid_guest'
      )
    }
  }

  private isExactGuest(record: RegisteredGuest): boolean {
    return (
      !record.guest.isDestroyed() &&
      !record.host.isDestroyed() &&
      record.guest.getType() === 'webview' &&
      record.guest.hostWebContents === record.host &&
      record.guest.session === this.expectedSession
    )
  }

  private removeRecordListeners(record: RegisteredGuest): void {
    record.guest.removeListener('destroyed', record.handleGuestDestroyed)
    record.host.removeListener('destroyed', record.handleHostDestroyed)
  }

  private assertUsable(): void {
    if (this.disposed) throw new BrowserTargetBrokerError('target_closed')
  }
}

function isValidSurfaceId(value: string): boolean {
  if (value.length === 0 || value.length > MAX_SURFACE_ID_LENGTH) return false
  for (let index = 0; index < value.length; index += 1) {
    const character = value.charCodeAt(index)
    if (character < 32 || character === 127) return false
  }
  return true
}
