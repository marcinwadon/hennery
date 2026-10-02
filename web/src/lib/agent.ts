// Who speaks in a transcript (F-14): the session's agent, never a name
// assumed for every session. The user is "You".

const KNOWN: Record<string, string> = { claude: 'Claude', codex: 'Codex' }

/** The agent's name as a transcript shows it: a known agent by its proper
 *  name, anything else as given, and "Agent" while the session is unknown. */
export function agentLabel(agent: string | undefined): string {
  if (!agent) return 'Agent'
  return Object.hasOwn(KNOWN, agent) ? KNOWN[agent] : agent
}

export const USER_LABEL = 'You'

/** An avatar's one letter: the label's first character, never anyone's
 *  initials. */
export function avatarLetter(label: string): string {
  return Array.from(label.trim())[0]?.toUpperCase() ?? '?'
}
