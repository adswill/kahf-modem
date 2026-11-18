// The push hub (MESH-SPEC.md step 10): what a Durable Object holding the relays' WebSockets
// does. Pure: sockets are objects with send(msg) and close(). Items for a relay without a
// socket wait in RAM for 60 s (the HTTPS poll fetches them); nothing is written to disk and
// message bodies never pass through (ids and sizes only). Not yet wired into wrangler.toml.

export const HOLD_MS = 60e3;
export const HEARTBEAT_MS = 30e3;
export const DEAD_AFTER_MS = 75e3; // two missed heartbeats and a bit

export class Hub {
  constructor({ onDown = () => {}, onUp = () => {} } = {}) {
    this.sockets = new Map(); // relay -> {socket, lastBeat}
    this.held = new Map(); // relay -> [{at, msg}]
    this.onDown = onDown;
    this.onUp = onUp;
  }

  // One live socket per relay key (I5): a second connection stops the first.
  connect(relay, socket, now) {
    const old = this.sockets.get(relay);
    if (old) {
      old.socket.send({ t: 'kicked' });
      old.socket.close();
    }
    this.sockets.set(relay, { socket, lastBeat: now });
    this.onUp(relay);
  }

  disconnect(relay, socket) {
    const cur = this.sockets.get(relay);
    if (cur && cur.socket === socket) {
      this.sockets.delete(relay);
      this.onDown(relay);
    }
  }

  heartbeat(relay, now) {
    const cur = this.sockets.get(relay);
    if (cur) cur.lastBeat = now;
  }

  // Push now if the relay has a socket, otherwise hold in RAM.
  push(relay, msg, now) {
    const cur = this.sockets.get(relay);
    if (cur) {
      cur.socket.send(msg);
      return 'pushed';
    }
    if (!this.held.has(relay)) this.held.set(relay, []);
    this.held.get(relay).push({ at: now, msg });
    return 'held';
  }

  // HTTPS fallback: what is waiting, nothing older than 60 s.
  poll(relay, now) {
    const items = (this.held.get(relay) ?? []).filter((h) => now - h.at <= HOLD_MS).map((h) => h.msg);
    this.held.delete(relay);
    return items;
  }

  // Call every few seconds: relays whose heartbeat stopped are dropped and reported down.
  tick(now) {
    for (const [relay, cur] of [...this.sockets]) {
      if (now - cur.lastBeat > DEAD_AFTER_MS) {
        cur.socket.close();
        this.sockets.delete(relay);
        this.onDown(relay);
      }
    }
    for (const [relay, list] of [...this.held]) {
      const keep = list.filter((h) => now - h.at <= HOLD_MS);
      if (keep.length) this.held.set(relay, keep);
      else this.held.delete(relay);
    }
  }
}
