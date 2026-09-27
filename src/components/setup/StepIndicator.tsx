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
    <div
      className="step-indicator"
      role="progressbar"
      aria-valuemin={1}
      aria-valuenow={displayIndex}
      aria-valuemax={total}
      aria-label={label}
    >
      {/* The rail. Each segment is a step: filled once reached, ringed while
          current. The step's name stays in the accessible tree (it is the
          only place the full six-step list is exposed) but is not painted —
          the label line below carries the one name that matters right now,
          and six names in 460px did not fit legibly at any size. */}
      <ol className="step-indicator-strip">
        {steps.map((name, i) => (
          <li
            key={name}
            className={`step-indicator-step${
              i < displayIndex ? " step-indicator-step--done" : ""
            }${i === safeCurrent ? " step-indicator-current" : ""}`}
            aria-current={i === safeCurrent ? "step" : undefined}
          >
            {name}
          </li>
        ))}
      </ol>
      <p className="step-indicator-label">
        <span>
          Step <b>{displayIndex}</b> of {total}
        </span>
        <span aria-hidden="true">·</span>
        <b>{steps[safeCurrent]}</b>
      </p>
      {/* Retained for the existing test hook; the rail above is now the
          visible progress affordance and this fill is no longer painted. */}
      <span
        className="step-indicator-fill"
        data-testid="step-indicator-fill"
        style={{ width: `${(displayIndex / total) * 100}%` }}
      />
    </div>
  );
}
