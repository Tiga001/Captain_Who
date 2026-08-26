import type { Session, WebContents } from 'electron'
import { ElectronGuestCdpTransport } from './ElectronGuestCdpTransport'
import {
  ElectronSurfaceGroupCdpTransport,
  type ElectronSurfaceGroupCdpTransportOptions
} from './ElectronSurfaceGroupCdpTransport'

const MAX_SURFACE_ID_LENGTH = 256

export type BrowserTargetBrokerErrorCode =
  | 'capacity_exceeded'
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
  generation: number
  guestWebContentsId: number
  host: WebContents
  surfaceId: string
}

export interface ManagedBrowserSurfaceIdentity {
  generation: number
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
  resolvePresentation?: BrowserSurfacePresentationResolver
  surfaceId?: string
  surfaceGeneration?: number
}

export interface BrowserSurfaceCdpPresentation {
  title: string
  url: string
}

export type BrowserSurfacePresentationResolver = (
  physicalUrl: string
) => BrowserSurfaceCdpPresentation | null

interface SurfaceClaim {
  generation: number
  guestId: number
}

export class BrowserTargetBroker {
  private static readonly MAX_REGISTERED_GUESTS = 32
  private readonly guests = new Map<number, RegisteredGuest>()
  private readonly surfaces = new Map<string, SurfaceClaim>()
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
    if (this.guests.size >= BrowserTargetBroker.MAX_REGISTERED_GUESTS) {
      throw new BrowserTargetBrokerError('capacity_exceeded')
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
    if (!Number.isSafeInteger(input.generation) || input.generation < 1) {
      throw new BrowserTargetBrokerError('surface_conflict')
    }
    const record = this.guests.get(input.guestWebContentsId)
    if (!record || record.host !== input.host) {
      throw new BrowserTargetBrokerError('guest_not_found')
    }
    this.revalidateRecord(record)

    const claim = this.surfaces.get(input.surfaceId)
    if (
      claim !== undefined &&
      (claim.guestId !== input.guestWebContentsId || claim.generation !== input.generation)
    ) {
      throw new BrowserTargetBrokerError('surface_conflict')
    }
    if (record.surfaceId !== undefined && record.surfaceId !== input.surfaceId) {
      throw new BrowserTargetBrokerError('surface_conflict')
    }

    record.surfaceId = input.surfaceId
    record.surfaceGeneration = input.generation
    this.surfaces.set(input.surfaceId, {
      generation: input.generation,
      guestId: input.guestWebContentsId
    })
  }

  unregisterManagedGuest(input: { guest: WebContents; host: WebContents }): void {
    const record = this.guests.get(input.guest.id)
    if (!record || record.guest !== input.guest || record.host !== input.host) return
    this.unregisterGuest(input.guest.id, input.guest, true)
  }

  async connect(surfaceId: string, generation?: number): Promise<ElectronGuestCdpTransport> {
    this.assertUsable()
    const record = this.getClaimedGuest(surfaceId, generation)
    this.revalidateRecord(record)
    if (record.connecting || record.activeTransport) {
      throw new BrowserTargetBrokerError('target_busy')
    }
    if (record.guest.isDestroyed()) throw new BrowserTargetBrokerError('target_closed')

    record.connecting = true
    try {
      const transport = new ElectronGuestCdpTransport(
        record.guest,
        (closedTransport) => {
          if (record.activeTransport === closedTransport) record.activeTransport = undefined
          if (record.pendingTransport === closedTransport) record.pendingTransport = undefined
        },
        (physicalUrl) => record.resolvePresentation?.(physicalUrl) ?? null
      )
      record.pendingTransport = transport
      await transport.attach()
      if (
        record.guest.isDestroyed() ||
        record.host.isDestroyed() ||
        this.disposed ||
        !this.isExactGuest(record) ||
        this.guests.get(record.guest.id) !== record ||
        record.surfaceId !== surfaceId ||
        this.surfaces.get(surfaceId)?.guestId !== record.guest.id ||
        this.surfaces.get(surfaceId)?.generation !== record.surfaceGeneration
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

  /**
   * Composes only exact claimed surfaces into one private BrowserContext transport. No Electron
   * target discovery occurs here; every delegate still passes the existing guest/host/partition
   * admission checks independently.
   */
  async connectSurfaceGroup(
    surfaces: readonly ManagedBrowserSurfaceIdentity[],
    options: ElectronSurfaceGroupCdpTransportOptions
  ): Promise<ElectronSurfaceGroupCdpTransport> {
    this.assertUsable()
    const group = new ElectronSurfaceGroupCdpTransport(options)
    try {
      for (const surface of surfaces) {
        await this.addSurfaceToGroup(group, surface)
      }
      return group
    } catch (error) {
      group.close()
      throw error
    }
  }

  async addSurfaceToGroup(
    group: ElectronSurfaceGroupCdpTransport,
    surface: ManagedBrowserSurfaceIdentity
  ): Promise<void> {
    this.assertUsable()
    if (group.hasSurface(surface.surfaceId, surface.generation)) return
    const transport = await this.connect(surface.surfaceId, surface.generation)
    try {
      await group.addSurface({ ...surface, transport })
    } catch (error) {
      transport.close()
      throw error
    }
  }

  releaseSurface(surfaceId: string, generation?: number): void {
    const claim = this.surfaces.get(surfaceId)
    if (claim === undefined || (generation !== undefined && claim.generation !== generation)) return
    const record = this.guests.get(claim.guestId)
    record?.pendingTransport?.close()
    record?.activeTransport?.close()
    if (record?.surfaceId === surfaceId) {
      record.resolvePresentation = undefined
      record.surfaceId = undefined
      record.surfaceGeneration = undefined
    }
    this.surfaces.delete(surfaceId)
  }

  setSurfacePresentationResolver(
    surfaceId: string,
    generation: number,
    resolver: BrowserSurfacePresentationResolver
  ): void {
    this.assertUsable()
    const record = this.getClaimedGuest(surfaceId, generation)
    record.resolvePresentation = resolver
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

  private getClaimedGuest(surfaceId: string, generation?: number): RegisteredGuest {
    const claim = this.surfaces.get(surfaceId)
    const record = claim === undefined ? undefined : this.guests.get(claim.guestId)
    if (
      !record ||
      record.surfaceId !== surfaceId ||
      record.surfaceGeneration !== claim?.generation ||
      (generation !== undefined && claim?.generation !== generation)
    ) {
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
    if (record.surfaceId) {
      const claim = this.surfaces.get(record.surfaceId)
      if (claim?.guestId === guestId && claim.generation === record.surfaceGeneration) {
        this.surfaces.delete(record.surfaceId)
      }
    }
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
