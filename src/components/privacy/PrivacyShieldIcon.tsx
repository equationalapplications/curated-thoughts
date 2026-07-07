import type { PrivacyMode } from "../../hooks/usePrivacyMode";

interface Props {
  mode: PrivacyMode;
}

export function PrivacyShieldIcon({ mode }: Props) {
  const className = `privacy-shield privacy-shield--${mode}`;

  if (mode === "strict") {
    return (
      <svg
        className={className}
        viewBox="0 0 16 16"
        width={16}
        height={16}
        aria-hidden="true"
      >
        <path
          fill="currentColor"
          d="M8 1 2 3.5v4.2c0 3.1 2.4 5.5 6 6.3 3.6-.8 6-3.2 6-6.3V3.5L8 1Z"
        />
      </svg>
    );
  }

  if (mode === "ephemeral") {
    return (
      <svg
        className={className}
        viewBox="0 0 16 16"
        width={16}
        height={16}
        aria-hidden="true"
      >
        <path
          fill="currentColor"
          d="M8 1 2 3.5v4.2c0 3.1 2.4 5.5 6 6.3 3.6-.8 6-3.2 6-6.3V3.5L8 1Z"
          opacity="0.45"
        />
        <path
          fill="currentColor"
          d="M8 1v12.7c3.6-.8 6-3.2 6-6.3V3.5L8 1Z"
        />
      </svg>
    );
  }

  return (
    <svg
      className={className}
      viewBox="0 0 16 16"
      width={16}
      height={16}
      aria-hidden="true"
    >
      <path
        fill="none"
        stroke="currentColor"
        strokeWidth="1.25"
        d="M8 1.6 2.8 3.8v3.9c0 2.7 2.1 4.8 5.2 5.5 3.1-.7 5.2-2.8 5.2-5.5V3.8L8 1.6Z"
      />
    </svg>
  );
}
