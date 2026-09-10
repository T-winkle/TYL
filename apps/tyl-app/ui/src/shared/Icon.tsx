const paths = {
  close: "m6 6 12 12M18 6 6 18",
  copy: "M9 9h11v11H9zM15 5V3H3v12h2",
  eye: "M2 12s3-7 10-7 10 7 10 7-3 7-10 7S2 12 2 12Zm10-3a3 3 0 1 0 0 6 3 3 0 0 0 0-6Z",
  eyeOff: "m3 3 18 18M10 5h2c7 0 10 7 10 7a18 18 0 0 1-3 4M6 6a19 19 0 0 0-4 6s3 7 10 7a12 12 0 0 0 5-1M9 9a4 4 0 0 0 6 6",
  replace: "M4 7h14m-4-4 4 4-4 4M20 17H6m4-4-4 4 4 4",
  sound: "m11 5-6 4H2v6h3l6 4V5Zm4 3a6 6 0 0 1 0 8m3-11a10 10 0 0 1 0 14",
  chevron: "m7 10 5 5 5-5",
  check: "m5 12 4 4L19 6",
  settings:
    "M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8Zm-8 4H2m20 0h-2M12 2v2m0 16v2M5 5l1.5 1.5m11 11L19 19M5 19l1.5-1.5m11-11L19 5",
  translate:
    "M3 5h12M9 2v3m-4 0c0 5 3 8 8 10M13 5c0 5-4 9-10 12m11 4 4-11 4 11m-6-4h4",
  network: "M3 7h16m-4-4 4 4-4 4M21 17H5m4-4-4 4 4 4",
  ai: "m12 3 3 6 6 3-6 3-3 6-3-6-6-3 6-3 3-6Z",
  up: "m6 14 6-6 6 6",
  down: "m6 10 6 6 6-6",
  sun: "M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8M12 2v2m0 16v2M2 12h2m16 0h2M5 5l1 1m12 12 1 1M5 19l1-1M18 6l1-1",
  moon: "M20 14A8 8 0 0 1 10 4a8 8 0 1 0 10 10Z",
  monitor: "M3 4h18v13H3zM8 21h8m-4-4v4",
} as const;

export type IconName = keyof typeof paths;
export function Icon(props: { name: IconName; size?: number }) {
  return (
    <svg
      width={props.size ?? 16}
      height={props.size ?? 16}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="1.65"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <path d={paths[props.name]} />
    </svg>
  );
}
