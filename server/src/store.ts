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

/** The HF backbone operations (src/backbone.js). Results are the objects' own JSON replies. */
export interface BackboneStore {
  msgClaim(origin: string, id: string, dest: string, nowMs: number): Promise<{ ok: true; duplicate: boolean }>;
  msgGetClaim(origin: string, id: string, nowMs: number): Promise<{ claim: { dest: string; status: string } | null }>;
  msgEnqueue(dest: string, message: object, nowMs: number): Promise<{ ok: true; until: number }>;
  msgPeek(dest: string, nowMs: number): Promise<{ messages: any[]; expired: { id: string; origin: string; dest: string }[]; remaining: number }>;
  msgHandout(dest: string, ids: string[], relay: string, nowMs: number): Promise<{ messages: any[] }>;
  msgComplete(
    dest: string,
    id: string,
    relay: string,
    nowMs: number,
  ): Promise<{ ok: true; found: boolean; dest?: string; remaining: number } | { ok: false; error: string }>;
  msgPutReceipt(origin: string, receipt: { id: string; dest: string; status: string; reason?: string }, nowMs: number): Promise<{ ok: true }>;
  msgTakeReceipts(origin: string, nowMs: number): Promise<{ receipts: any[] }>;
  idxNoteHeard(relay: string, heard: object[], nowMs: number): Promise<{ ok: true; recorded: number; ignored: number }>;
  idxGetHeard(callsign: string, nowMs: number): Promise<{ heard: { callsign: string; relay: string; at: number; snr?: number } | null }>;
  idxWant(dest: string, until: number): Promise<{ ok: true }>;
  idxUnwant(dest: string): Promise<{ ok: true }>;
  idxWanted(nowMs: number): Promise<{ wanted: string[] }>;
  idxRate(relay: string, nowMs: number, limit: number): Promise<{ ok: boolean }>;
}

/** KV expires entries on its own after 48 h; refreshing a report resets the timer. */
const TTL_SECONDS = 48 * 60 * 60;

const recordKey = (reporter: string, heard: string) => `rec:${reporter}>${heard}`;
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
  BackboneStore {
  const mail = <T>(call: string, op: string, args: object) => callObject<T>(env.MAILBOXES, call, op, args);
  const index = <T>(op: string, args: object) => callObject<T>(env.BACKBONE, "index", op, args);
  return {
    msgClaim: (origin, id, dest, nowMs) => mail(origin, "claim", { id, dest, nowMs }),
    msgGetClaim: (origin, id, nowMs) => mail(origin, "getClaim", { id, nowMs }),
    msgEnqueue: (dest, message, nowMs) => mail(dest, "enqueue", { message, nowMs }),
    msgPeek: (dest, nowMs) => mail(dest, "peek", { nowMs }),
    msgHandout: (dest, ids, relay, nowMs) => mail(dest, "handout", { ids, relay, nowMs }),
    msgComplete: (dest, id, relay, nowMs) => mail(dest, "complete", { id, relay, nowMs }),
    msgPutReceipt: (origin, receipt, nowMs) => mail(origin, "putReceipt", { ...receipt, nowMs }),
    msgTakeReceipts: (origin, nowMs) => mail(origin, "takeReceipts", { nowMs }),
    idxNoteHeard: (relay, heard, nowMs) => index("noteHeard", { relay, heard, nowMs }),
    idxGetHeard: (callsign, nowMs) => index("getHeard", { callsign, nowMs }),
    idxWant: (dest, until) => index("want", { dest, until }),
    idxUnwant: (dest) => index("unwant", { dest }),
    idxWanted: (nowMs) => index("wanted", { nowMs }),
    idxRate: (relay, nowMs, limit) => index("rate", { relay, nowMs, limit }),

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
    },

    bookSchedule(area, relay, requests, nowMs) {
      return callObject(env.AREAS, area, "book", { area, relay, requests, nowMs });
    },
  };
}
