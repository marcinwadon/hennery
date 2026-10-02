export type Seg = { t: string; c?: string }

export function tokJSON(line: string): Seg[] {
  const re = /("(?:\\.|[^"\\])*")(?=\s*:)|("(?:\\.|[^"\\])*")|\b(true|false|null)\b|(-?\d+\.?\d*)|([{}\[\],:])/g
  const out: Seg[] = []
  let last = 0
  let m: RegExpExecArray | null
  while ((m = re.exec(line))) {
    if (m.index > last) out.push({ t: line.slice(last, m.index) })
    if (m[1]) out.push({ t: m[1], c: 'tk-key' })
    else if (m[2]) out.push({ t: m[2], c: m[2].includes('redacted') ? 'tk-redact' : 'tk-str' })
    else if (m[3]) out.push({ t: m[3], c: 'tk-kw' })
    else if (m[4]) out.push({ t: m[4], c: 'tk-num' })
    else if (m[5]) out.push({ t: m[5], c: 'tk-punc' })
    last = re.lastIndex
  }
  if (last < line.length) out.push({ t: line.slice(last) })
  return out
}
