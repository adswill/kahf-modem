// Shared types. Kept minimal so no @cloudflare/workers-types dependency is needed.

/** Minimal slice of the Workers KV binding that the store uses. */
export interface KVNamespace {
  get(key: string, type: "json"): Promise<unknown>;
  put(key: string, value: string, options?: { expirationTtl?: number }): Promise<void>;
  delete(key: string): Promise<void>;
  list(options?: { prefix?: string; cursor?: string; limit?: number }): Promise<{
    keys: { name: string }[];
    list_complete: boolean;
    cursor?: string;
  }>;
}

/** Minimal slice of a Durable Object namespace binding. */
export interface DurableObjectNamespace {
  idFromName(name: string): unknown;
  get(id: unknown): { fetch(url: string, init?: { method?: string; headers?: Record<string, string>; body?: string }): Promise<{ ok: boolean; status: number; json(): Promise<unknown> }> };
}

export interface Env {
  /** Station records. */
  STATIONS: KVNamespace;
  /** Base64 Ed25519 public key of the owner's release key. Set as a secret, never in the code. */
  RELEASE_PUBLIC_KEY?: string;
  /** One object per callsign: owner and relay registration (src/objects.js CallsignObject). */
  CALLSIGNS: DurableObjectNamespace;
  /** One object per one-degree area: live relays and slot bookings (src/objects.js AreaObject). */
  AREAS: DurableObjectNamespace;
}

/**
 * A position as the reporter chose to share it.
 * precise: the point (latitude, longitude), rounded by the client to about 200 m.
 * area: a centre point and radius_km; the true position lies within the circle.
 */
export type Location =
  | { mode: "precise"; latitude: number; longitude: number }
  | { mode: "area"; latitude: number; longitude: number; radius_km: number };

/** One stored report. */
export interface StationRecord {
  reporter_call: string;
  reporter_location: Location;
  heard_call: string;
  heard_location: Location;
  heard_at: string;
  frequency_khz: number;
  mode: string;
  /** Base64 Ed25519 public key of the reporter. Never sent to clients. */
  public_key: string;
  /** Server clock, milliseconds since the epoch. */
  refreshed_at: number;
}

/** Which key currently owns a reporter callsign. */
export interface OwnerRecord {
  public_key: string;
  refreshed_at: number;
  /** Heard callsign to signing time (ms) of recent removals, so a removal cannot be replayed. */
  removed?: Record<string, number>;
}

/** A registered relay: its callsign, its one-degree area and its key. */
export interface RelayRecord {
  callsign: string;
  area: string;
  /** Base64 Ed25519 public key of the relay. */
  public_key: string;
  /** Server clock, milliseconds since the epoch. */
  refreshed_at: number;
}

/** A registered relay in the map index: callsign, one-degree area and last registration. */
export interface RelayIndexRecord {
  callsign: string;
  area: string;
  /** Server clock, milliseconds since the epoch. */
  refreshed_at: number;
}

/** The current release announcement (signed by the release key). */
export interface ReleaseRecord {
  version: string;
  published_at: string;
  /** True when apps older than this release must update before they transmit. */
  required: boolean;
  notes_url: string;
}

/** The slots last planned for one relay, kept so other relays avoid them. */
export interface SlotsRecord {
  callsign: string;
  area: string;
  slots: { channel: number; start: number; duration: number; direction: "rx" | "tx" }[];
  /** Server clock, milliseconds since the epoch. */
  refreshed_at: number;
}

/** What GET /stations returns for each record. */
export interface PublicStation {
  reporter_call: string;
  reporter_location: Location;
  heard_call: string;
  heard_location: Location;
  heard_at: string;
  frequency_khz: number;
  mode: string;
  refreshed_at: string;
}
