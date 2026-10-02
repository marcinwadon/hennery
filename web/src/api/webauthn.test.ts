import { describe, expect, it } from 'vitest'
import { credentialJson, creationOptions, requestOptions, toBase64url, wasDismissed } from './webauthn'

const bytes = (...b: number[]) => new Uint8Array(b).buffer

describe('options without the browser’s JSON parsers', () => {
  it('decodes a registration’s base64url fields', () => {
    const { publicKey } = creationOptions({
      publicKey: {
        challenge: toBase64url(bytes(1, 2, 3)),
        rp: { id: 'hennery.example', name: 'hennery' },
        user: { id: toBase64url(bytes(9, 9)), name: 'owner', displayName: 'owner' },
        excludeCredentials: [{ type: 'public-key', id: toBase64url(bytes(255, 254)) }],
      },
    })
    expect(new Uint8Array(publicKey!.challenge as ArrayBuffer)).toEqual(new Uint8Array([1, 2, 3]))
    expect(new Uint8Array(publicKey!.user.id as ArrayBuffer)).toEqual(new Uint8Array([9, 9]))
    expect(new Uint8Array(publicKey!.excludeCredentials![0].id as ArrayBuffer)).toEqual(new Uint8Array([255, 254]))
    expect(publicKey!.rp.id).toBe('hennery.example')
  })

  it('decodes a login’s base64url fields', () => {
    const { publicKey } = requestOptions({
      publicKey: { challenge: 'AQID', allowCredentials: [{ type: 'public-key', id: '_-8' }], userVerification: 'required' },
    })
    expect(new Uint8Array(publicKey!.challenge as ArrayBuffer)).toEqual(new Uint8Array([1, 2, 3]))
    expect(new Uint8Array(publicKey!.allowCredentials![0].id as ArrayBuffer)).toEqual(new Uint8Array([255, 239]))
    expect(publicKey!.userVerification).toBe('required')
  })

  it('refuses options without publicKey', () => {
    expect(() => requestOptions({})).toThrow()
  })
})

describe('credentialJson', () => {
  it('uses the browser’s toJSON when there is one', () => {
    const credential = { toJSON: () => ({ id: 'x' }) } as unknown as Credential
    expect(credentialJson(credential)).toEqual({ id: 'x' })
  })

  it('encodes an assertion by hand otherwise', () => {
    const credential = {
      id: 'cred',
      rawId: bytes(255),
      type: 'public-key',
      response: { clientDataJSON: bytes(1), authenticatorData: bytes(2), signature: bytes(3), userHandle: null },
      getClientExtensionResults: () => ({}),
    } as unknown as Credential
    expect(credentialJson(credential)).toEqual({
      id: 'cred',
      rawId: '_w',
      type: 'public-key',
      response: { clientDataJSON: 'AQ', authenticatorData: 'Ag', signature: 'Aw' },
      clientExtensionResults: {},
    })
  })

  it('refuses no credential', () => {
    expect(() => credentialJson(null)).toThrow()
  })
})

it('tells a dismissed prompt from a failure', () => {
  expect(wasDismissed(new DOMException('no', 'NotAllowedError'))).toBe(true)
  expect(wasDismissed(new DOMException('no', 'SecurityError'))).toBe(false)
  expect(wasDismissed(new Error('x'))).toBe(false)
})
