// One item's error boundary (frontend spec §6.4): an item that throws while
// rendering shows a line in its place, and the rest of the transcript
// stays. It is keyed by the item's id; a new version of the item renders
// it again. A tool call's possible fabrication (F-13) outlives any failure
// of its renderer: the fallback still shows the banner, as text.
import { Component, type ReactNode } from 'react'

interface Props {
  version: number
  /** A tool call's `fabricated`, as text: shown even when it fails. */
  fabricated?: string
  children: ReactNode
}

interface State {
  failed: boolean
  version: number
}

export default class ItemBoundary extends Component<Props, State> {
  state: State = { failed: false, version: this.props.version }

  static getDerivedStateFromError(): Partial<State> {
    return { failed: true }
  }

  static getDerivedStateFromProps(props: Props, state: State): Partial<State> | null {
    return props.version !== state.version ? { failed: false, version: props.version } : null
  }

  render() {
    if (this.state.failed) {
      const { fabricated } = this.props
      return (
        <>
          {fabricated !== undefined && (
            <div className="fab-warn" role="alert">
              <span className="fab-warn-tag">Possible fabricated tool call</span>
              <span>{fabricated}</span>
            </div>
          )}
          <div className="divider item-failed">Could not render this item</div>
        </>
      )
    }
    return this.props.children
  }
}
