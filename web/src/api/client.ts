// The typed transport (client view spec §6, frontend spec §3): every API
// call goes through `request`, which answers the three refusals the same
// way everywhere.
//
// - 401 `unauthenticated`: to the login screen, which returns here.
// - 403 `step_up_required`: the step-up dialog, then the request once more.
//   Safe to send again: every step-up check runs before its handler does
//   anything (`require_step_up` is a route or handler layer; the handlers
//   that check for themselves check before they read or write), so the
//   refused attempt changed nothing. Never a third time, never another
//   dialog.
// - Anything else not 2xx: an `ApiFailure` with the server's code.
//
// `mode` is left at fetch's default, `cors`: with it a same-origin `POST`
// carries `Origin`, which the server requires, under the page's
// `Referrer-Policy: no-referrer`.
import { ApiFailure, Unauthenticated, retryAfterSeconds } from './errors'
import { loginHref } from './next'

export interface ClientDeps {
  fetch: typeof fetch
  /** Navigate inside the app (`history.pushState`), never to another origin. */
  navigate: (to: string) => void
  /** The current path and query, without the fragment. */
  here: () => { pathname: string; search: string }
  /** Open the step-up dialog: resolves once stepped up, rejects with
   *  `StepUpCancelled` when dismissed. */
  stepUp: () => Promise<void>
}

export interface RequestOptions {
  /** `false` on the step-up routes themselves: a 403 there is an error. */
  stepUp?: boolean
  signal?: AbortSignal
}

/** Screens whose own 401s are answers, not a reason to go and sign in. */
const SIGN_IN_PAGES = ['/login', '/setup']

export class Client {
  private readonly deps: ClientDeps
  private steppingUp: Promise<void> | null = null

  constructor(deps: ClientDeps) {
    this.deps = deps
  }

  /** `method path` with a JSON body, answered as JSON (`undefined` for an
   *  empty answer). */
  async request<T>(method: string, path: string, body?: unknown, options: RequestOptions = {}): Promise<T> {
    let response = await this.send(method, path, body, options.signal)
    if (options.stepUp !== false && (await isStepUpRequired(response))) {
      await this.stepUpOnce()
      response = await this.send(method, path, body, options.signal)
    }
    return this.answer<T>(response)
  }

  /** A `GET` whose answer the caller reads itself (the SSE helper): the 401
   *  rule applies; any other status is the caller's. */
  async open(path: string, headers: Record<string, string>, signal: AbortSignal): Promise<Response> {
    const response = await this.deps.fetch(path, { method: 'GET', headers, credentials: 'same-origin', signal })
    if (response.status === 401) await this.unauthenticated(response)
    return response
  }

  private send(method: string, path: string, body: unknown, signal?: AbortSignal): Promise<Response> {
    const headers: Record<string, string> = { Accept: 'application/json' }
    const init: RequestInit = { method, headers, credentials: 'same-origin', signal }
    if (body !== undefined) {
      headers['Content-Type'] = 'application/json'
      init.body = JSON.stringify(body)
    }
    return this.deps.fetch(path, init)
  }

  /** One dialog for every request refused while it is open. */
  private stepUpOnce(): Promise<void> {
    if (!this.steppingUp) {
      this.steppingUp = this.deps.stepUp().finally(() => {
        this.steppingUp = null
      })
    }
    return this.steppingUp
  }

  private async answer<T>(response: Response): Promise<T> {
    if (response.ok) {
      if (response.status === 204 || response.status === 205) return undefined as T
      const type = response.headers.get('content-type') ?? ''
      return (type.includes('application/json') ? await response.json() : await response.text()) as T
    }
    if (response.status === 401) await this.unauthenticated(response)
    const retryAfter = response.status === 429 ? retryAfterSeconds(response.headers.get('retry-after')) : undefined
    throw new ApiFailure(response.status, await errorBody(response), retryAfter)
  }

  /** A 401 `unauthenticated` sends the browser to sign in, and back here
   *  afterwards; on the sign-in screens it is only an error. */
  private async unauthenticated(response: Response): Promise<void> {
    const body = await errorBody(response.clone())
    const { pathname, search } = this.deps.here()
    if (body?.code !== 'unauthenticated' || SIGN_IN_PAGES.includes(pathname)) return
    this.deps.navigate(loginHref(pathname + search))
    throw new Unauthenticated()
  }
}

async function errorBody(response: Response): Promise<{ code?: string; message?: string } | undefined> {
  try {
    const body: unknown = await response.json()
    if (body && typeof body === 'object') {
      const { code, message } = body as Record<string, unknown>
      return {
        code: typeof code === 'string' ? code : undefined,
        message: typeof message === 'string' ? message : undefined,
      }
    }
  } catch {
    // Not JSON: the status speaks for itself.
  }
  return undefined
}

async function isStepUpRequired(response: Response): Promise<boolean> {
  if (response.status !== 403) return false
  return (await errorBody(response.clone()))?.code === 'step_up_required'
}
