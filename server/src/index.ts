// Station map Worker entry point. All routing lives in routes.js, which takes
// the store as an argument; this file wires the store to KV (station records) and
// to the Durable Objects (owners, relay registrations, slot bookings).
//   GET  /           home page
//   GET  /map        live map
//   GET  /stations   station records as JSON
//   GET  /zones      busy channels per area and channel pools
//   POST /report     signed reception report
//   POST /remove     signed removal
//   POST /register   signed relay registration
//   POST /schedule   signed slot request from a registered relay
//   GET  /map-data   relays and stations as JSON
//   GET  /release    current release announcement
//   POST /release    signed release announcement (owner's release key)
import { handleRequest } from "./routes.js";
import { kvStore, objectStore } from "./store";
import type { Env } from "./types";

export { CallsignObject, AreaObject } from "./objects.js";

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    return handleRequest(
      request,
      { ...kvStore(env.STATIONS), ...objectStore(env) },
      Date.now(),
      { releaseKey: env.RELEASE_PUBLIC_KEY },
    );
  },
};
