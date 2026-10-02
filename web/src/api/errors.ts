// What a refused request becomes (client view spec §6): one error type that
// carries the server's code, and a plain message per known code.
import type { ApiError } from '../generated/protocol'

/** A plain message for each code the UI explains itself. Any other code
 *  shows the server's own message, as text. */
const MESSAGES: Record<string, string> = {
  hat_ambiguous: 'This path matches more than one hat: pick the path exactly as its rule names it.',
  cwd_moved: 'The session’s directory has moved since it last ran.',
  hat_mismatch: 'This directory now belongs to another hat than the session’s.',
  presumed_parked: 'Its host has been away and may still run it: close the session first.',
  images_unsupported: 'This agent takes no images.',
  host_offline: 'The host is offline.',
  host_installing: 'The host is still installing its agents.',
  already_set_up: 'hennery is set up already: sign in instead.',
  invalid_setup_token:
    'This setup link is used or expired. Run `hennery admin setup-url` on the collector for a new one.',
  origin_mismatch: 'The public URL must be the address this page is open at.',
  setup_required: 'hennery is not set up yet. Run `hennery admin setup-url` on the collector for the setup link.',
  invalid_password: 'Wrong password.',
  rate_limited: 'Too many attempts. Wait, then try again.',
  no_passkeys: 'No passkey is registered.',
  passkeys_unavailable: 'Passkeys need a public URL with a host name, not an IP address.',
  passkey_refused: 'The passkey was not accepted.',
  invalid_ceremony: 'The passkey prompt expired. Try again.',
  already_registered: 'This authenticator holds a passkey for hennery already.',
  step_up_required: 'Confirm it is you, then try again.',
  unauthenticated: 'Sign in first.',
  too_many_codes: 'Sixteen pairing codes are live already: wait for one to expire.',
  name_taken: 'Another hat has this name.',
  hat_purging: 'This hat is being purged.',
  hat_is_default:
    'This hat is the default for new hosts, or a host’s default hat: make another hat that default first.',
  resolve_unsupported: 'This host cannot resolve paths yet: update hennery on it.',
  no_answer: 'The host did not answer in time. Try again.',
  busy: 'The host is busy. Try again.',
  too_many_subscriptions: 'Remove a device before adding another: 32 at most.',
  endpoint_taken: 'This browser receives notifications for another account.',
}

export class ApiFailure extends Error {
  readonly status: number
  readonly code: string
  /** The server's own message, shown as text when the code is unknown. */
  readonly serverMessage: string
  /** `Retry-After`, in seconds, on a 429. */
  readonly retryAfter?: number

  constructor(status: number, body: Partial<ApiError> | undefined, retryAfter?: number) {
    const code = body?.code ?? `http_${status}`
    const serverMessage = body?.message ?? `The request failed (${status}).`
    // Own keys only: a code like `constructor` is the server's, not Object's.
    super(Object.hasOwn(MESSAGES, code) ? MESSAGES[code] : serverMessage)
    this.name = 'ApiFailure'
    this.status = status
    this.code = code
    this.serverMessage = serverMessage
    this.retryAfter = retryAfter
  }
}

/** A 401: the browser was sent to the login screen. */
export class Unauthenticated extends Error {
  constructor() {
    super('Sign in first.')
    this.name = 'Unauthenticated'
  }
}

/** The step-up dialog was dismissed: the refused request is not sent again. */
export class StepUpCancelled extends Error {
  constructor() {
    super('Not confirmed.')
    this.name = 'StepUpCancelled'
  }
}

/** The message to show for any error a request can end in. */
export function messageOf(err: unknown): string {
  if (err instanceof Error) return err.message
  return 'Something went wrong.'
}

/** `Retry-After` as whole seconds: absent or not a number of seconds, 60. */
export function retryAfterSeconds(value: string | null): number {
  if (value !== null && /^\d+$/.test(value.trim())) return Number(value.trim())
  return 60
}
