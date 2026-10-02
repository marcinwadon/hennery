// The one place an agent's Markdown becomes DOM (frontend spec §6.4; client
// view spec §5.3): messages and thinking share this plugin stack.
//
//   remark-gfm       tables, task lists, strikethrough, autolinks
//   remark-breaks    a single newline is a line break, as in a terminal
//   htmlAsText       raw HTML in the text stays text: it is never parsed
//                    (there is no rehype-raw), and it is never dropped
//   rehype-sanitize  the default (GitHub) schema over what Markdown made,
//                    without a second id prefix (see `schema`)
//   rehype-highlight fenced code, highlight.js' common languages only, no
//                    guessing; an unknown language is left plain, never thrown.
//                    Version 7's options are `detect`, `languages` (default:
//                    lowlight's `common`), `plainText`, `prefix` and `subset`;
//                    it has no `ignoreMissing`: a fence in a language it does
//                    not have becomes a message on the file, not an error.
//
// Links open in a new tab without an opener (a footnote's stays on the page). An image never loads on its own:
// it is a link with its alt text. react-markdown's default `urlTransform`
// stays, so a `javascript:` URL loses its href.
import type { ComponentProps } from 'react'
import ReactMarkdown, { type Components } from 'react-markdown'
import rehypeHighlight from 'rehype-highlight'
import rehypeSanitize, { defaultSchema, type Options as SanitizeSchema } from 'rehype-sanitize'
import remarkBreaks from 'remark-breaks'
import remarkGfm from 'remark-gfm'

interface MdNode {
  type: string
  value?: string
  children?: MdNode[]
}

/** A remark plugin: every raw HTML node (block or inline) becomes a text
 *  node with the same source, so it renders as the characters it is. */
export function htmlAsText() {
  const visit = (node: MdNode) => {
    if (node.type === 'html') node.type = 'text'
    node.children?.forEach(visit)
  }
  return visit
}

// rehype-highlight registers its languages each time it is set up, and
// react-markdown sets its plugins up for every message it renders. Set up
// once, its transformer is shared by every message instead.
const highlight = rehypeHighlight({ detect: false })
function sharedHighlight() {
  return highlight
}

// The GitHub schema, but no second prefix on ids. The ids Markdown makes
// here are all footnotes' (remark-rehype): a note's and its reference's
// carry `user-content-`, and their links point at those (prefixed again,
// every footnote link would miss); the notes' heading is
// `id="footnote-label"`, unprefixed. No other id or `name` can come in,
// and that, not the prefix, is what makes `clobberPrefix: ''` safe: raw HTML
// never becomes elements (`htmlAsText`, no rehype-raw).
const schema: SanitizeSchema = { ...defaultSchema, clobberPrefix: '' }

/** An image in Markdown: never loaded, a link with its alt text. `src` has
 *  been through `urlTransform` (an unsafe one is empty) and the sanitizer
 *  (an empty one is gone); either way there is no link. */
export function MarkdownImage({ src, alt }: ComponentProps<'img'> & { node?: unknown }) {
  const label = alt ? `Image: ${alt}` : 'Image'
  return typeof src === 'string' && src ? (
    <a href={src} target="_blank" rel="noopener noreferrer" className="md-img-link">
      {label}
    </a>
  ) : (
    <span className="md-img-link">{label}</span>
  )
}

const components: Components = {
  // A link within the message (a footnote) stays on the page.
  a: ({ node: _node, ...props }: ComponentProps<'a'> & { node?: unknown }) =>
    typeof props.href === 'string' && props.href.startsWith('#') ? (
      <a {...props} />
    ) : (
      <a {...props} target="_blank" rel="noopener noreferrer" />
    ),
  img: MarkdownImage,
}

export default function Markdown({ children }: { children: string }) {
  return (
    <ReactMarkdown
      remarkPlugins={[remarkGfm, remarkBreaks, htmlAsText]}
      rehypePlugins={[[rehypeSanitize, schema], sharedHighlight]}
      components={components}
    >
      {children}
    </ReactMarkdown>
  )
}
