// The hat selector (frontend spec §5): every hat, or one. The hats are the
// server's; the app never computes a session's hat.
import type { HatItem } from '../generated/protocol'

interface Props {
  hats: readonly HatItem[] | null
  /** The selected hat's id, `''` for every hat. */
  hat: string
  onHat: (hat: string) => void
}

export default function HatSwitch({ hats, hat, onHat }: Props) {
  const choices = [{ id: '', name: 'All hats' }, ...(hats ?? []).map((h) => ({ id: h.id, name: h.name }))]
  return (
    <div className="hat-switch list-hats" role="group" aria-label="Hat">
      {choices.map((c) => (
        <button
          key={c.id || '*'}
          type="button"
          className={'chip' + (hat === c.id ? ' on' : '')}
          aria-pressed={hat === c.id}
          onClick={() => onHat(c.id)}
        >
          <bdi>{c.name}</bdi>
        </button>
      ))}
    </div>
  )
}
