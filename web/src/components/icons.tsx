import type { JSX } from 'preact';

export interface IconProps {
  size?: number;
  class?: string;
}

function svgProps({ size = 16, class: className }: IconProps): JSX.SVGAttributes<SVGSVGElement> {
  return {
    width: size,
    height: size,
    viewBox: '0 0 24 24',
    fill: 'none',
    stroke: 'currentColor',
    'stroke-width': 1.6,
    'stroke-linecap': 'round',
    'stroke-linejoin': 'round',
    'aria-hidden': 'true',
    focusable: 'false',
    class: className,
  };
}

export function PlusIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <path d="M12 5v14M5 12h14" />
    </svg>
  );
}

export function CloseIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <path d="M6 6l12 12M18 6L6 18" />
    </svg>
  );
}

export function ChevronRightIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <path d="M9 6l6 6-6 6" />
    </svg>
  );
}

export function ChevronDownIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <path d="M6 9l6 6 6-6" />
    </svg>
  );
}

export function SidebarIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <rect x="3" y="4" width="18" height="16" rx="2" />
      <path d="M9 4v16" />
    </svg>
  );
}

export function PanelIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <rect x="3" y="4" width="18" height="16" rx="2" />
      <path d="M15 4v16" />
    </svg>
  );
}

export function SettingsIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.7 1.7 0 00.34 1.88l.06.06a2 2 0 11-2.83 2.83l-.06-.06A1.7 1.7 0 0015 19.4a1.7 1.7 0 00-1 .6 1.7 1.7 0 00-.4 1.1V21a2 2 0 11-4 0v-.1A1.7 1.7 0 009 19.4a1.7 1.7 0 00-1.88.34l-.06.06a2 2 0 11-2.83-2.83l.06-.06A1.7 1.7 0 004.6 15a1.7 1.7 0 00-.6-1 1.7 1.7 0 00-1.1-.4H3a2 2 0 110-4h.1A1.7 1.7 0 004.6 9a1.7 1.7 0 00-.34-1.88l-.06-.06a2 2 0 112.83-2.83l.06.06A1.7 1.7 0 009 4.6a1.7 1.7 0 001-.6 1.7 1.7 0 00.4-1.1V3a2 2 0 114 0v.1A1.7 1.7 0 0015 4.6a1.7 1.7 0 001.88-.34l.06-.06a2 2 0 112.83 2.83l-.06.06A1.7 1.7 0 0019.4 9c.1.37.32.7.6 1 .3.26.68.4 1.1.4H21a2 2 0 110 4h-.1a1.7 1.7 0 00-1.5.6z" />
    </svg>
  );
}

export function RefreshIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <path d="M20 11a8 8 0 10-2.3 5.7" />
      <path d="M20 4v7h-7" />
    </svg>
  );
}

export function EditIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <path d="M12 20h9" />
      <path d="M16.5 3.5a2.121 2.121 0 013 3L7 19l-4 1 1-4L16.5 3.5z" />
    </svg>
  );
}

export function CopyIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <rect x="9" y="9" width="11" height="11" rx="2" />
      <path d="M5 15V6a2 2 0 012-2h9" />
    </svg>
  );
}

export function SendIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <path d="M4 12l16-8-6 16-3-6-7-2z" />
    </svg>
  );
}

export function StopIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <rect x="6" y="6" width="12" height="12" rx="2" fill="currentColor" stroke="none" />
    </svg>
  );
}

export function SearchIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <circle cx="11" cy="11" r="7" />
      <path d="M20 20l-3.5-3.5" />
    </svg>
  );
}

export function FolderIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <path d="M3 6a2 2 0 012-2h4l2 2h8a2 2 0 012 2v10a2 2 0 01-2 2H5a2 2 0 01-2-2z" />
    </svg>
  );
}

export function FileIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <path d="M6 3h8l4 4v14H6z" />
      <path d="M14 3v4h4" />
    </svg>
  );
}

export function ChatIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <path d="M20 11.5a7.5 7.5 0 0 1-7.5 7.5 8.1 8.1 0 0 1-3.2-.7L4 20l1.7-4.8a7.5 7.5 0 1 1 14.3-3.7Z" />
    </svg>
  );
}

export function GitIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <circle cx="6" cy="6" r="2.5" />
      <circle cx="6" cy="18" r="2.5" />
      <circle cx="18" cy="8" r="2.5" />
      <path d="M6 8.5v7M18 10.5c0 4-5 4-9 4" />
    </svg>
  );
}

export function TerminalIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <rect x="3" y="4" width="18" height="16" rx="2" />
      <path d="M7 9l3 3-3 3M13 15h4" />
    </svg>
  );
}

export function AgentsIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <circle cx="12" cy="8" r="3" />
      <path d="M5.5 20a6.5 6.5 0 0113 0" />
      <path d="M4 8h2M18 8h2" />
    </svg>
  );
}

export function CheckIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <path d="M5 12l4.5 4.5L19 7" />
    </svg>
  );
}

export function AlertIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <path d="M12 4l9 16H3z" />
      <path d="M12 10v4M12 17h.01" />
    </svg>
  );
}

export function KebabIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <circle cx="12" cy="6" r="1" fill="currentColor" />
      <circle cx="12" cy="12" r="1" fill="currentColor" />
      <circle cx="12" cy="18" r="1" fill="currentColor" />
    </svg>
  );
}

export function ChevronLeftIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <path d="M15 6l-6 6 6 6" />
    </svg>
  );
}

export function MenuIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <path d="M4 7h16M4 12h16M4 17h16" />
    </svg>
  );
}
/* ---- icons added for the v3 workspace design -------------------------- */

export function PaletteIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <path d="M12 2.5a9.5 9.5 0 1 0 0 19h1.4a2.4 2.4 0 0 0 1.6-4.2 1.5 1.5 0 0 1 .9-2.6h2a4 4 0 0 0 4-4A10 10 0 0 0 12 2.5Z" />
      <circle cx="7.5" cy="11" r="1" />
      <circle cx="10" cy="6.7" r="1" />
      <circle cx="15.5" cy="7.5" r="1" />
      <circle cx="17" cy="11.4" r="1" />
    </svg>
  );
}

export function UploadIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <path d="M12 16V4m-5 5 5-5 5 5M4 17v3h16v-3" />
    </svg>
  );
}

export function DownloadIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <path d="M12 3v12m-5-5 5 5 5-5M4 17v3h16v-3" />
    </svg>
  );
}

export function TrashIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <path d="M4 7h16M10 4h4m-8 3 1 14h10l1-14M10 11v7m4-7v7" />
    </svg>
  );
}

export function SparkIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <path d="m12 2 1.8 6.2L20 10l-6.2 1.8L12 18l-1.8-6.2L4 10l6.2-1.8L12 2Z" />
    </svg>
  );
}

export function ArrowRightIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <path d="M5 12h14m-6-6 6 6-6 6" />
    </svg>
  );
}

export function CircleCheckIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <circle cx="12" cy="12" r="9" />
      <path d="m8 12 3 3 5-6" />
    </svg>
  );
}

export function LockIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <rect x="4" y="10" width="16" height="11" rx="2" />
      <path d="M8 10V7a4 4 0 1 1 8 0v3" />
    </svg>
  );
}

export function PaperclipIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <path d="m21 11.5-8.9 8.9a6 6 0 0 1-8.5-8.5l9.6-9.6a4 4 0 0 1 5.7 5.7l-10 10a2 2 0 0 1-2.8-2.8l9.2-9.2" />
    </svg>
  );
}

export function FileCodeIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <path d="M13 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9Z" />
      <path d="M13 2v7h7M10 13l-2 2 2 2m4-4 2 2-2 2" />
    </svg>
  );
}

export function FolderOpenIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <path d="M3 6a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v3M3 8v9a2 2 0 0 0 2 2h13l3-8H7l-4 8" />
    </svg>
  );
}

export function BoltIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <path d="m13 2-9 12h7l-1 8 10-12h-7l0-8Z" />
    </svg>
  );
}

export function BranchIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <circle cx="6" cy="5" r="2" />
      <circle cx="6" cy="19" r="2" />
      <circle cx="18" cy="7" r="2" />
      <path d="M6 7v10M18 9a6 6 0 0 1-6 6H6" />
    </svg>
  );
}

export function ThumbUpIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <path d="M7 10v11H4a2 2 0 0 1-2-2v-7a2 2 0 0 1 2-2Zm0 0 5-8a3 3 0 0 1 2 4l-1 4h6a3 3 0 0 1 3 3l-1.3 6a3 3 0 0 1-3 2H7" />
    </svg>
  );
}

export function CpuIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <rect x="5" y="5" width="14" height="14" rx="2" />
      <rect x="9" y="9" width="6" height="6" />
      <path d="M9 2v3m6-3v3M9 19v3m6-3v3M19 9h3m-3 6h3M2 9h3m-3 6h3" />
    </svg>
  );
}

export function EyeIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <path d="M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12Z" />
      <circle cx="12" cy="12" r="3" />
    </svg>
  );
}

export function LayersIcon(props: IconProps): JSX.Element {
  return (
    <svg {...svgProps(props)}>
      <path d="m12 3 9 5-9 5-9-5 9-5Z" />
      <path d="m3 12 9 5 9-5" />
      <path d="m3 16 9 5 9-5" />
    </svg>
  );
}

/**
 * The icons a plugin may name in its manifest's panel `icon` field.
 *
 * A fixed, first-party set on purpose: a plugin supplies a *name*, the app
 * decides what that draws. An arbitrary SVG from a plugin would be untrusted
 * markup in the app's origin, which is exactly what the panel design avoids.
 * An unknown name draws `LayersIcon`, so a plugin from a newer version still
 * gets a tab rather than a missing glyph.
 */
const PLUGIN_PANEL_ICONS: Record<string, (props: IconProps) => JSX.Element> = {
  check: CheckIcon,
  'circle-check': CircleCheckIcon,
  clipboard: EditIcon,
  copy: CopyIcon,
  layers: LayersIcon,
  list: LayersIcon,
  listbox: EditIcon,
  eye: EyeIcon,
  search: SearchIcon,
  spark: SparkIcon,
  folder: FolderIcon,
  'folder-open': FolderOpenIcon,
  file: FileIcon,
  'file-code': FileCodeIcon,
  git: GitIcon,
  branch: BranchIcon,
  terminal: TerminalIcon,
  agents: AgentsIcon,
  bolt: BoltIcon,
  cpu: CpuIcon,
  chat: ChatIcon,
  lock: LockIcon,
  alert: AlertIcon,
  upload: UploadIcon,
  download: DownloadIcon,
  thumb: ThumbUpIcon,
};

/** Every icon name a plugin may use, for the `plugins` doc and settings UI. */
export const PLUGIN_PANEL_ICON_NAMES: readonly string[] = Object.keys(PLUGIN_PANEL_ICONS).sort();

/** Resolve a plugin's icon name, falling back to a neutral glyph. */
export function pluginPanelIcon(
  name: string | null | undefined,
  props: IconProps = {},
): JSX.Element {
  const Icon =
    (name ? PLUGIN_PANEL_ICONS[name.trim().toLowerCase()] : undefined) ?? LayersIcon;
  return <Icon {...props} />;
}
