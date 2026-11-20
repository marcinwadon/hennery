import type { SessionConfig } from '../generated/protocol'

export type ProtocolPreference = NonNullable<SessionConfig['protocol']>
export function protocolValue(value: unknown): ProtocolPreference | undefined {
  return value === 'v1' || value === 'v2' || value === 'v2-v1' ? value : undefined
}
