/** Passkeys in the browser: the server speaks base64url JSON, the WebAuthn API speaks ArrayBuffers. */

type Json = Record<string, unknown>;

function toBuffer(b64url: string): ArrayBuffer {
  const b64 = b64url.replace(/-/g, "+").replace(/_/g, "/");
  const raw = atob(b64 + "=".repeat((4 - (b64.length % 4)) % 4));
  const out = new Uint8Array(raw.length);
  for (let i = 0; i < raw.length; i++) out[i] = raw.charCodeAt(i);
  return out.buffer;
}

function toB64url(buf: ArrayBuffer | null | undefined): string | null {
  if (!buf) return null;
  let s = "";
  new Uint8Array(buf).forEach((b) => (s += String.fromCharCode(b)));
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function withBufferIds(list: unknown): PublicKeyCredentialDescriptor[] | undefined {
  if (!Array.isArray(list)) return undefined;
  return (list as (Json & { id: string })[]).map((c) => ({ ...c, id: toBuffer(c.id) }) as unknown as PublicKeyCredentialDescriptor);
}

/** This browser can use passkeys here (TVs and plain-http pages can't). */
export function passkeysSupported(): boolean {
  return typeof window !== "undefined" && window.isSecureContext && typeof window.PublicKeyCredential === "function" && !!navigator.credentials;
}

/** Create a passkey from the server's options; returns the JSON the server verifies. */
export async function createPasskey(options: unknown): Promise<unknown> {
  const pk = (options as { publicKey: Json & { challenge: string; user: Json & { id: string }; excludeCredentials?: unknown } }).publicKey;
  const cred = (await navigator.credentials.create({
    publicKey: {
      ...pk,
      challenge: toBuffer(pk.challenge),
      user: { ...pk.user, id: toBuffer(pk.user.id) },
      excludeCredentials: withBufferIds(pk.excludeCredentials),
    } as unknown as PublicKeyCredentialCreationOptions,
  })) as PublicKeyCredential | null;
  if (!cred) throw new Error("No passkey was created.");
  const r = cred.response as AuthenticatorAttestationResponse;
  return {
    id: cred.id,
    rawId: toB64url(cred.rawId),
    type: cred.type,
    extensions: cred.getClientExtensionResults(),
    response: { attestationObject: toB64url(r.attestationObject), clientDataJSON: toB64url(r.clientDataJSON) },
  };
}

/** Sign the server's challenge with a passkey; returns the JSON the server verifies. */
export async function usePasskey(options: unknown): Promise<unknown> {
  const pk = (options as { publicKey: Json & { challenge: string; allowCredentials?: unknown } }).publicKey;
  const cred = (await navigator.credentials.get({
    publicKey: {
      ...pk,
      challenge: toBuffer(pk.challenge),
      allowCredentials: withBufferIds(pk.allowCredentials),
    } as unknown as PublicKeyCredentialRequestOptions,
  })) as PublicKeyCredential | null;
  if (!cred) throw new Error("No passkey was used.");
  const r = cred.response as AuthenticatorAssertionResponse;
  return {
    id: cred.id,
    rawId: toB64url(cred.rawId),
    type: cred.type,
    extensions: cred.getClientExtensionResults(),
    response: {
      authenticatorData: toB64url(r.authenticatorData),
      clientDataJSON: toB64url(r.clientDataJSON),
      signature: toB64url(r.signature),
      userHandle: toB64url(r.userHandle),
    },
  };
}

/** The person closed the browser's passkey sheet: not worth an error message. */
export function passkeyCancelled(e: unknown): boolean {
  return e instanceof DOMException && (e.name === "NotAllowedError" || e.name === "AbortError");
}
