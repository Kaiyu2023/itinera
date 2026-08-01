import type { Leg } from '../api/types';

const MODE_PATH: Record<Leg['mode'], React.ReactNode> = {
  walk: (
    <>
      <circle cx="12" cy="4.5" r="2" />
      <path d="M10.2 8.2l3.1 2.5 2.6 4M13.3 10.7l-1.5 4.2-4.2 5.2M11.8 14.9l4.3 5.1M10.2 8.2L7.4 12" />
    </>
  ),
  transit: (
    <>
      <path d="M6 4.5h12a2 2 0 012 2v8.2a2.6 2.6 0 01-2.6 2.6H8.6A2.6 2.6 0 016 14.7zM6 10.4h14M9.5 20.5l1.2-3.2M16.1 20.5l-1.2-3.2M4.5 20.5h15" />
      <circle cx="9.2" cy="14" r=".8" fill="currentColor" stroke="none" />
      <circle cx="16.8" cy="14" r=".8" fill="currentColor" stroke="none" />
    </>
  ),
  drive: (
    <>
      <path d="M4 15.8V11l2-5h12l2 5v4.8a1.5 1.5 0 01-1.5 1.5h-13A1.5 1.5 0 014 15.8zM6 11h12M7.5 17.3v2M16.5 17.3v2" />
      <circle cx="7.5" cy="14" r="1" />
      <circle cx="16.5" cy="14" r="1" />
    </>
  ),
  flight: (
    <path d="M3 13l7.5-2.2V4.5c0-1 .7-1.8 1.5-1.8s1.5.8 1.5 1.8v6.3L21 13v2l-7.5-.8v4.1l2.3 1.5v1.5L12 20.4l-3.8.9v-1.5l2.3-1.5v-4.1L3 15z" />
  ),
};

export function ModeGlyph({ mode, label }: { mode: Leg['mode']; label?: string }) {
  return (
    <svg
      className={`mode-glyph mode-${mode}`}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.7}
      strokeLinecap="round"
      strokeLinejoin="round"
      role={label ? 'img' : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
    >
      {MODE_PATH[mode]}
    </svg>
  );
}
