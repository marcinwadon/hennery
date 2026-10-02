// The icon set: stroke icons drawn on a 24-unit grid, in currentColor.
import type { ReactNode } from 'react'

type IconProps = { size?: number; className?: string }

function mk(paths: ReactNode, vb = 24, sw = 1.8) {
  return ({ size = 18, className }: IconProps) => (
    <svg
      width={size}
      height={size}
      viewBox={`0 0 ${vb} ${vb}`}
      fill="none"
      stroke="currentColor"
      strokeWidth={sw}
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      aria-hidden="true"
    >
      {paths}
    </svg>
  )
}

export const Icon = {
  Plus: mk(<path d="M12 5v14M5 12h14" />),
  Search: mk(
    <g>
      <circle cx="11" cy="11" r="7" />
      <path d="m20 20-3.2-3.2" />
    </g>,
  ),
  Folder: mk(<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />),
  ArrowUp: mk(<path d="M12 19V5M6 11l6-6 6 6" />),
  X: mk(<path d="M6 6l12 12M18 6 6 18" />),
  Check: mk(<path d="M5 12.5 10 17.5 19.5 7" />, 24, 2.4),
  Wrench: mk(<path d="M15.5 7.5a4 4 0 0 1-5.2 5.2L5 18l1 1 5.3-5.3a4 4 0 0 0 5.2-5.2l-2.3 2.3-2-2z" />),
  Live: mk(<circle cx="12" cy="12" r="3.2" fill="currentColor" stroke="none" />),
  Terminal: mk(
    <g>
      <path d="M5 8l3 3-3 3M11 16h6" />
      <rect x="2.5" y="4.5" width="19" height="15" rx="2" />
    </g>,
  ),
  Cpu: mk(
    <g>
      <rect x="6" y="6" width="12" height="12" rx="2" />
      <path d="M9 1.5v3M15 1.5v3M9 19.5v3M15 19.5v3M1.5 9h3M1.5 15h3M19.5 9h3M19.5 15h3" />
    </g>,
  ),
  Sparkle: mk(<path d="M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8z" />),
  Bolt: mk(<path d="M13 2 4 14h6l-1 8 9-12h-6z" />),
  Branch: mk(
    <g>
      <circle cx="6" cy="6" r="2.4" />
      <circle cx="6" cy="18" r="2.4" />
      <circle cx="18" cy="7" r="2.4" />
      <path d="M6 8.4v7.2M8.4 6.4c6 .6 8 2 8 5.6v1.6" />
    </g>,
  ),
  Chevron: mk(<path d="m9 6 6 6-6 6" />),
  Menu: mk(<path d="M4 7h16M4 12h16M4 17h16" />),
  List: mk(
    <g>
      <path d="M8 6h13M8 12h13M8 18h13M3.5 6h.01M3.5 12h.01M3.5 18h.01" />
    </g>,
  ),
  Columns: mk(
    <g>
      <rect x="3" y="4" width="7" height="16" rx="1.5" />
      <rect x="14" y="4" width="7" height="16" rx="1.5" />
    </g>,
  ),
  Bell: mk(
    <g>
      <path d="M18 8A6 6 0 0 0 6 8c0 7-3 9-3 9h18s-3-2-3-9" />
      <path d="M13.73 21a2 2 0 0 1-3.46 0" />
    </g>,
  ),
  BellOff: mk(
    <g>
      <path d="M13.73 21a2 2 0 0 1-3.46 0" />
      <path d="M18.63 13A17.89 17.89 0 0 1 18 8" />
      <path d="M6.26 6.26A5.86 5.86 0 0 0 6 8c0 7-3 9-3 9h14" />
      <path d="M18 8a6 6 0 0 0-9.33-5" />
      <line x1="1" y1="1" x2="23" y2="23" />
    </g>,
  ),
  Paperclip: mk(
    <path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48" />,
  ),
  File: mk(
    <g>
      <path d="M6 2.5h7l5 5V21a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1V3.5a1 1 0 0 1 1-1Z" />
      <path d="M13 2.5V8h5" />
    </g>,
  ),
  Shield: mk(<g><path d="M12 3l7 3v5c0 5-3.2 8.5-7 10-3.8-1.5-7-5-7-10V6z" /></g>),
}
