interface Props {
  current: number;
  total: number;
  steps: string[];
}

export function StepIndicator({ current, total, steps }: Props) {
  if (total < 1) return null;
  const safeCurrent = Math.max(0, Math.min(current, total - 1));
  const displayIndex = safeCurrent + 1;
  const label = `Step ${displayIndex} of ${total}: ${steps[safeCurrent] ?? ""}`;
  return (
    <div className="step-indicator" aria-label="Setup progress">
      <ol className="step-indicator-strip">
        {steps.map((name, i) => (
          <li
            key={name}
            className={`step-indicator-step${i === safeCurrent ? " step-indicator-current" : ""}`}
            aria-current={i === safeCurrent ? "step" : undefined}
          >
            {name}
          </li>
        ))}
      </ol>
      <p className="step-indicator-label">{label}</p>
      <div
        className="step-indicator-bar"
        role="progressbar"
        aria-valuemin={1}
        aria-valuenow={displayIndex}
        aria-valuemax={total}
        aria-label={label}
      >
        <span
          className="step-indicator-fill"
          data-testid="step-indicator-fill"
          style={{ width: `${(displayIndex / total) * 100}%` }}
        />
      </div>
    </div>
  );
}
