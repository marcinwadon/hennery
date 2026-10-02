// A server time (RFC 3339), shown in the browser's own locale and zone, with
// the exact value a hover away. One the browser cannot read is shown as
// the server sent it, escaped.
import { visible } from '../lib/text'

export default function When({ at }: { at: string }) {
  const date = new Date(at)
  const shown = visible(at)
  return (
    <time dateTime={shown} title={shown}>
      {Number.isNaN(date.getTime()) ? shown : date.toLocaleString()}
    </time>
  )
}
