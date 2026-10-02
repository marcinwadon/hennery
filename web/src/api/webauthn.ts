// WebAuthn's JSON forms (plan 3c, "What the frontend must do"): the server
// sends `{ publicKey }` with base64url fields and takes the credential back
// as `toJSON()` makes it. Browsers without `parse…FromJSON` or `toJSON` get
// the fields converted here.

type Json = Record<string, unknown>

function fromBase64url(value: string): ArrayBuffer {
  const base64 = value.replace(/-/g, '+').replace(/_/g, '/')
  const padded = base64 + '='.repeat((4 - (base64.length % 4)) % 4)
  const binary = atob(padded)
  const bytes = new Uint8Array(binary.length)
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i)
  return bytes.buffer
}

export function toBase64url(buffer: ArrayBuffer | ArrayBufferView): string {
  const bytes = buffer instanceof ArrayBuffer ? new Uint8Array(buffer) : new Uint8Array(buffer.buffer, buffer.byteOffset, buffer.byteLength)
  let binary = ''
  for (const b of bytes) binary += String.fromCharCode(b)
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')
}

function publicKeyOf(options: unknown): Json {
  const publicKey = (options as Json | null)?.publicKey
  if (!publicKey || typeof publicKey !== 'object') throw new Error('The server sent no passkey options.')
  return publicKey as Json
}

function withIds(list: unknown): unknown {
  if (!Array.isArray(list)) return list
  return list.map((c: Json) => ({ ...c, id: fromBase64url(c.id as string) }))
}

type WithJsonParsers = typeof PublicKeyCredential & {
  parseCreationOptionsFromJSON?: (json: unknown) => PublicKeyCredentialCreationOptions
  parseRequestOptionsFromJSON?: (json: unknown) => PublicKeyCredentialRequestOptions
}

function parsers(): WithJsonParsers | undefined {
  return typeof PublicKeyCredential === 'undefined' ? undefined : (PublicKeyCredential as WithJsonParsers)
}

/** Whether this browser can use passkeys at all. */
export function passkeysSupported(): boolean {
  return typeof window !== 'undefined' && typeof window.PublicKeyCredential !== 'undefined' && !!navigator.credentials
}

/** A registration's options, from `register/start`'s `options`. */
export function creationOptions(options: unknown): CredentialCreationOptions {
  const json = publicKeyOf(options)
  const parse = parsers()?.parseCreationOptionsFromJSON
  if (parse) return { publicKey: parse(json) }
  const user = json.user as Json
  return {
    publicKey: {
      ...(json as object),
      challenge: fromBase64url(json.challenge as string),
      user: { ...user, id: fromBase64url(user.id as string) },
      excludeCredentials: withIds(json.excludeCredentials),
    } as PublicKeyCredentialCreationOptions,
  }
}

/** A login's or step-up's options, from its `start`'s `options`. */
export function requestOptions(options: unknown): CredentialRequestOptions {
  const json = publicKeyOf(options)
  const parse = parsers()?.parseRequestOptionsFromJSON
  if (parse) return { publicKey: parse(json) }
  return {
    publicKey: {
      ...(json as object),
      challenge: fromBase64url(json.challenge as string),
      allowCredentials: withIds(json.allowCredentials),
    } as PublicKeyCredentialRequestOptions,
  }
}

/** The credential as its `finish` takes it. */
export function credentialJson(credential: Credential | null): unknown {
  if (!credential) throw new Error('No passkey was chosen.')
  const pk = credential as PublicKeyCredential & { toJSON?: () => unknown }
  if (typeof pk.toJSON === 'function') return pk.toJSON()
  const response = pk.response as AuthenticatorResponse & Json
  const out: Json = { clientDataJSON: toBase64url(response.clientDataJSON) }
  for (const field of ['attestationObject', 'authenticatorData', 'signature', 'userHandle']) {
    const value = response[field]
    if (value instanceof ArrayBuffer) out[field] = toBase64url(value)
  }
  return {
    id: pk.id,
    rawId: toBase64url(pk.rawId),
    type: pk.type,
    response: out,
    clientExtensionResults: pk.getClientExtensionResults?.() ?? {},
  }
}

/** Whether `err` is the browser's answer to a dismissed prompt: nothing to
 *  send, the ceremony just expires. */
export function wasDismissed(err: unknown): boolean {
  return err instanceof DOMException && err.name === 'NotAllowedError'
}
