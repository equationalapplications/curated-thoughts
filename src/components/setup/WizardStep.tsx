import type { ReactNode } from "react";
import { useId } from "react";

export interface WizardStepProps {
  title: string;
  subtitle?: string;
  children: ReactNode;
  onBack?: () => void;
  onNext?: () => void;
  nextLabel?: string;
  nextDisabled?: boolean;
  onSkip?: () => void;
  skipLabel?: string;
  isLoading?: boolean;
}

const DEFAULT_NEXT = "Continue";
const DEFAULT_SKIP = "Skip — take me to the app";

export function WizardStep({
  title,
  subtitle,
  children,
  onBack,
  onNext,
  nextLabel = DEFAULT_NEXT,
  nextDisabled,
  onSkip,
  skipLabel = DEFAULT_SKIP,
  isLoading,
}: WizardStepProps) {
  const titleId = useId();
  return (
    <section
      className="wizard-step"
      aria-labelledby={titleId}
      data-testid="setup-mode-wizard"
    >
      <header className="wizard-step-header">
        <h2 id={titleId} className="wizard-step-title">{title}</h2>
        {subtitle && <p className="wizard-step-subtitle">{subtitle}</p>}
      </header>
      <div className="wizard-step-body">{children}</div>
      {(onBack || onNext || onSkip) && (
        <footer className="wizard-step-footer">
          {onBack && (
            <button type="button" className="wizard-step-btn wizard-step-btn--back" onClick={onBack}>
              Back
            </button>
          )}
          {onSkip && (
            <button type="button" className="wizard-step-btn wizard-step-btn--skip" onClick={onSkip}>
              {skipLabel}
            </button>
          )}
          {onNext && (
            <button
              type="button"
              className="wizard-step-btn wizard-step-btn--next"
              onClick={onNext}
              disabled={nextDisabled || isLoading}
              aria-busy={isLoading}
            >
              {nextLabel}
              {isLoading && (
                <span className="wizard-step-spinner" data-testid="wizard-step-spinner" aria-hidden="true" />
              )}
            </button>
          )}
        </footer>
      )}
    </section>
  );
}
