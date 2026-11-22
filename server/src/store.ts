// The only module that touches storage. Station records live in Workers KV (they are
// read in bulk and expire by themselves). Every check-and-write on ownership, relay
// registration and slot bookings goes to a Durable Object, which does the check and
// the write in one call, so two requests cannot both succeed. To change a backend,
// replace the methods below with another implementation of StationStore.
import type { DurableObjectNamespace, Env, KVNamespace, RelayIndexRecord, ReleaseRecord, RelayRecord, StationRecord } from "./types";

/** Outcome of a claim. `previousArea` is set when a relay moves from another area. */
export type Claim = { ok: true; previousArea?: string | null } | { ok: false; error: string };

export interface ScheduleRequest {
  relay: string;
  channel: number;
  duration: number;
  direction: "rx" | "tx";
  urgent: boolean;
}

export interface StationStore {
  get(reporterCall: string, heardCall: string): Promise<StationRecord | null>;
  put(record: StationRecord): Promise<void>;
  remove(reporterCall: string, heardCall: string): Promise<void>;
  listAll(): Promise<StationRecord[]>;
  /** Claims a reporter callsign for a key (or refreshes it). Refused while another key holds it. */
  claimOwner(reporterCall: string, publicKey: string, nowMs: number): Promise<Claim>;
  /** Uses one removal (heard callsign, signing time) once; refuses a replay. */
  claimRemoval(reporterCall: string, heardCall: string, publicKey: string, signedAtMs: number, nowMs: number): Promise<Claim>;
  /** Registers a relay callsign for a key and area. Refused while another key holds it. */
  claimRelay(callsign: string, area: string, publicKey: string, nowMs: number): Promise<Claim>;
  getRelay(callsign: string): Promise<RelayRecord | null>;
  /** Marks a relay as live in an area. */
  joinArea(area: string, callsign: string, nowMs: number): Promise<void>;
  /** Forgets a relay in an area (it moved away). */
  leaveArea(area: string, callsign: string): Promise<void>;
  /** Indexes a registered relay for the map (48 h from the last registration). */
  putRelayIndex(callsign: string, area: string, nowMs: number): Promise<void>;
  /** Every indexed relay, expired or not (routes filter by age). */
  listRelays(): Promise<RelayIndexRecord[]>;
  /** The current release announcement, or null. */
  getRelease(): Promise<ReleaseRecord | null>;
  /** Replaces the current release announcement. */
  putRelease(record: ReleaseRecord): Promise<void>;
  /** Plans the relay's slots in the area against the others' bookings and stores them. */
  bookSchedule(
    area: string,
    relay: string,
    requests: ScheduleRequest[],
    nowMs: number,
  ): Promise<{ ok: true; slots: unknown[] } | { ok: false; error: string }>;
}

/** The relay mesh (src/mesh.js): one object for the world. Results are the object's own JSON replies. */
export interface MeshStore {
  meshOp(op: "frames" | "poll" | "status", args: object): Promise<any>;
  /** Hands a WebSocket upgrade (already authenticated) to the mesh object. */
  meshSocket(call: string, request: Request): Promise<Response>;
}

/** KV expires entries on its own after 48 h; refreshing a report resets the timer. */
const TTL_SECONDS = 48 * 60 * 60;

const recordKey = (reporter: string, heard: string) => `rec:${reporter}>${heard}`;
/** Name of the one AreaObject that plans slots for every relay. */
const WORLD_SLOTS = "world-slots";
const relayIndexKey = (callsign: string) => `relayidx:${callsign}`;
/** The one current release announcement. */
const RELEASE_KEY = "release:current";

/** Every value under a prefix, read in pages of up to 1000 keys. */
async function listValues<T>(ns: KVNamespace, prefix: string): Promise<T[]> {
  const keys: string[] = [];
  let cursor: string | undefined;
  do {
    const page = await ns.list({ prefix, cursor, limit: 1000 });
    for (const k of page.keys) keys.push(k.name);
    cursor = page.list_complete ? undefined : page.cursor;
  } while (cursor);
  const values = await Promise.all(keys.map((k) => ns.get(k, "json")));
  return values.filter((v): v is T => v !== null && typeof v === "object");
}

/**
 * Station records in Workers KV. KV is a good fit: few writes, many reads, and entries
 * expire by themselves. Reads can lag writes by up to about a minute, which is fine for a map.
 */
export function kvStore(
  ns: KVNamespace,
): Pick<StationStore, "get" | "put" | "remove" | "listAll" | "putRelayIndex" | "listRelays" | "getRelease" | "putRelease"> {
  return {
    async get(reporter, heard) {
      return (await ns.get(recordKey(reporter, heard), "json")) as StationRecord | null;
    },

    async put(record) {
      await ns.put(recordKey(record.reporter_call, record.heard_call), JSON.stringify(record), {
        expirationTtl: TTL_SECONDS,
      });
    },

    async remove(reporter, heard) {
      await ns.delete(recordKey(reporter, heard));
    },

    async listAll() {
      return listValues<StationRecord>(ns, "rec:");
    },

    /** Relay index for the map. Expires with the station records, 48 h after the last registration. */
    async putRelayIndex(callsign, area, nowMs) {
      const rec: RelayIndexRecord = { callsign, area, refreshed_at: nowMs };
      await ns.put(relayIndexKey(callsign), JSON.stringify(rec), { expirationTtl: TTL_SECONDS });
    },

    async listRelays() {
      return listValues<RelayIndexRecord>(ns, "relayidx:");
    },

    async getRelease() {
      return (await ns.get(RELEASE_KEY, "json")) as ReleaseRecord | null;
    },

    async putRelease(record) {
      await ns.put(RELEASE_KEY, JSON.stringify(record));
    },
  };
}

/** One operation on a Durable Object: POST { op, args } to the object named by `name`. */
async function callObject<T>(ns: DurableObjectNamespace, name: string, op: string, args: object): Promise<T> {
  const stub = ns.get(ns.idFromName(name));
  const res = await stub.fetch("https://object.internal/op", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ op, args }),
  });
  if (!res.ok) throw new Error(`durable object ${op} failed with status ${res.status}`);
  return (await res.json()) as T;
}

/**
 * Ownership, registration and slot bookings. One object per callsign (CALLSIGNS) holds
 * the owner and the relay registration; one per area (AREAS) holds the live relays and
 * the planned slots of that area.
 */
export function objectStore(env: Env): Pick<
  StationStore,
  "claimOwner" | "claimRemoval" | "claimRelay" | "getRelay" | "joinArea" | "leaveArea" | "bookSchedule"
> &
  MeshStore {
  return {
    meshOp: (op, args) => callObject(env.MESH, "mesh", op, args),
    meshSocket(call, request) {
      const stub = env.MESH.get(env.MESH.idFromName("mesh"));
      const r = new Request("https://object.internal/socket", request);
      r.headers.set("x-relay", call);
      return stub.fetch(r);
    },

    claimOwner(reporter, publicKey, nowMs) {
      return callObject<Claim>(env.CALLSIGNS, reporter, "claimOwner", { publicKey, nowMs });
    },

    claimRemoval(reporter, heard, publicKey, signedAtMs, nowMs) {
      return callObject<Claim>(env.CALLSIGNS, reporter, "claimRemoval", { publicKey, heardCall: heard, signedAtMs, nowMs });
    },

    claimRelay(callsign, area, publicKey, nowMs) {
      return callObject<Claim>(env.CALLSIGNS, callsign, "claimRelay", { callsign, area, publicKey, nowMs });
    },

    getRelay(callsign) {
      return callObject<RelayRecord | null>(env.CALLSIGNS, callsign, "getRelay", {});
    },

    async joinArea(area, callsign, nowMs) {
      await callObject(env.AREAS, area, "join", { callsign, nowMs });
    },

    async leaveArea(area, callsign) {
      await callObject(env.AREAS, area, "leave", { callsign });
      await callObject(env.AREAS, WORLD_SLOTS, "leave", { callsign });
    },

    bookSchedule(area, relay, requests, nowMs) {
      // One object for the whole world: HF is global, so transmit slots are planned together.
      return callObject(env.AREAS, WORLD_SLOTS, "book", { area, relay, requests, nowMs });
    },
  };
}
