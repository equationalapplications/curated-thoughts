export function SkipLink({
  targetId,
  label = "Skip to main content",
}: {
  targetId: string;
  label?: string;
}) {
  return (
    <a href={`#${targetId}`} className="skip-link">
      {label}
    </a>
  );
}
