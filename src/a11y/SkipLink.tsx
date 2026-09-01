export function SkipLink({
  targetId,
  label = "Skip to main content",
}: {
  targetId: string;
  label?: string;
}) {
  return (
    <a
      href={`#${targetId}`}
      className="skip-link"
      onClick={(e) => {
        const target = document.getElementById(targetId);
        if (!target) return; // no destination mounted — fall through to hash nav
        e.preventDefault();
        target.scrollIntoView();
        // The destination carries tabIndex={-1} (AppShell) so programmatic
        // focus actually lands; preventScroll avoids double-scrolling.
        target.focus({ preventScroll: true });
      }}
    >
      {label}
    </a>
  );
}
