interface LogoProps {
  class?: string;
  label?: string;
}

/** Theme-aware UI logo. Static packaged icons keep using logo.svg. */
export function Logo(props: LogoProps) {
  return (
    <svg
      class={`tyl-logo${props.class ? ` ${props.class}` : ""}`}
      viewBox="0 0 64 64"
      role={props.label ? "img" : undefined}
      aria-label={props.label}
      aria-hidden={props.label ? undefined : "true"}
    >
      <rect class="tyl-logo-bg" x="1" y="1" width="62" height="62" rx="18" />
      <path class="tyl-logo-fg" d="M16 19h32v8H36v21h-8V27H16v-8Z" />
      <path class="tyl-logo-detail" d="M41 34h7v14h-7V34Z" />
      <path class="tyl-logo-detail" d="M16 19h12v8H16z" />
    </svg>
  );
}
