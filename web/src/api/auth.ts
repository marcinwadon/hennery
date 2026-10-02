// The auth routes (kernel spec §3, §8; plan 3c) and capabilities, typed.
import type {
  AuthSessionItem,
  CapabilitiesResponse,
  PasskeyCeremony,
  PasskeyItem,
  SetupResponse,
} from '../generated/protocol'
import type { Client } from './client'
import { credentialJson, creationOptions, requestOptions } from './webauthn'

export interface SetupInput {
  token: string
  password: string
  publicUrl: string
  defaultHatName: string
}

export function setUp(client: Client, input: SetupInput): Promise<SetupResponse> {
  return client.request('POST', '/api/setup', {
    token: input.token,
    password: input.password,
    public_url: input.publicUrl,
    default_hat_name: input.defaultHatName,
  })
}

export function logIn(client: Client, password: string): Promise<void> {
  return client.request('POST', '/api/auth/login', { password })
}

export function logOut(client: Client): Promise<void> {
  return client.request('POST', '/api/auth/logout')
}

export function capabilities(client: Client): Promise<CapabilitiesResponse> {
  return client.request('GET', '/api/capabilities')
}

export function stepUpWithPassword(client: Client, password: string): Promise<void> {
  return client.request('POST', '/api/auth/step-up/password', { password }, { stepUp: false })
}

export function signedInSessions(client: Client): Promise<AuthSessionItem[]> {
  return client.request('GET', '/api/auth/sessions')
}

export function passkeys(client: Client): Promise<PasskeyItem[]> {
  return client.request('GET', '/api/auth/passkeys')
}

/** Start a passkey login: its options, to hand to `navigator.credentials.get`
 *  in the click that follows (a prompt needs the click's user activation). */
export function startPasskeyLogin(client: Client): Promise<PasskeyCeremony> {
  return client.request('POST', '/api/auth/passkeys/login/start')
}

export function finishPasskeyLogin(client: Client, ceremony: PasskeyCeremony, credential: Credential | null): Promise<void> {
  return client.request('POST', '/api/auth/passkeys/login/finish', {
    ceremony_id: ceremony.ceremony_id,
    credential: credentialJson(credential),
  })
}

export function startPasskeyStepUp(client: Client): Promise<PasskeyCeremony> {
  return client.request('POST', '/api/auth/step-up/passkey/start', undefined, { stepUp: false })
}

export function finishPasskeyStepUp(client: Client, ceremony: PasskeyCeremony, credential: Credential | null): Promise<void> {
  return client.request(
    'POST',
    '/api/auth/step-up/passkey/finish',
    { ceremony_id: ceremony.ceremony_id, credential: credentialJson(credential) },
    { stepUp: false },
  )
}

/** Register a passkey: start, the browser's prompt, finish. Needs a
 *  stepped-up session; a refusal opens the step-up dialog and retries. */
export async function registerPasskey(client: Client, label: string): Promise<PasskeyItem> {
  const ceremony = await client.request<PasskeyCeremony>('POST', '/api/auth/passkeys/register/start', { label })
  const credential = await navigator.credentials.create(creationOptions(ceremony.options))
  return client.request('POST', '/api/auth/passkeys/register/finish', {
    ceremony_id: ceremony.ceremony_id,
    credential: credentialJson(credential),
  })
}

/** The options of a ceremony, for `navigator.credentials.get`. */
export function getOptions(ceremony: PasskeyCeremony): CredentialRequestOptions {
  return requestOptions(ceremony.options)
}
