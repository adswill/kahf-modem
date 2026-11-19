// Ed25519 signing of the project's plans and band tables (WebCrypto, works in Workers and Node 20).
// The private key is a 32 byte seed kept as a Cloudflare secret (PROJECT_KEY, base64); the apps carry
// the public key. Nothing here stores a key.

const PKCS8_PREFIX = Uint8Array.from([0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20]);

const b64 = (bytes) => btoa(String.fromCharCode(...bytes));
const unb64 = (s) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));

/** A fresh keypair: { seed, publicKey }, both base64. For tooling and tests. */
export async function generateKeypair() {
  const seed = crypto.getRandomValues(new Uint8Array(32));
  const key = await importSeed(seed);
  const jwk = await crypto.subtle.exportKey("jwk", key);
  const pub = Uint8Array.from(atob(jwk.x.replace(/-/g, "+").replace(/_/g, "/")), (c) => c.charCodeAt(0));
  return { seed: b64(seed), publicKey: b64(pub) };
}

async function importSeed(seed) {
  const der = new Uint8Array(PKCS8_PREFIX.length + 32);
  der.set(PKCS8_PREFIX);
  der.set(seed, PKCS8_PREFIX.length);
  return crypto.subtle.importKey("pkcs8", der, { name: "Ed25519" }, true, ["sign"]);
}

/** Signs `text` (UTF-8) with the base64 seed; returns the base64 signature. */
export async function signText(seedB64, text) {
  const key = await importSeed(unb64(seedB64));
  return b64(new Uint8Array(await crypto.subtle.sign({ name: "Ed25519" }, key, new TextEncoder().encode(text))));
}

/** True when `sigB64` is a valid signature of `text` by the base64 public key. */
export async function verifyText(publicKeyB64, text, sigB64) {
  try {
    const key = await crypto.subtle.importKey("raw", unb64(publicKeyB64), { name: "Ed25519" }, false, ["verify"]);
    return await crypto.subtle.verify({ name: "Ed25519" }, key, unb64(sigB64), new TextEncoder().encode(text));
  } catch {
    return false;
  }
}
