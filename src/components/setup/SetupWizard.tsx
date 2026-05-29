import { useState } from "react";
import { StepWelcome } from "./StepWelcome";
import { StepFastembed } from "./StepFastembed";
import { StepModel } from "./StepModel";
import { StepDone } from "./StepDone";

interface Props {
  onComplete: () => void | Promise<void>;
  initialStep?: number;
}

export function SetupWizard({ onComplete, initialStep = 0 }: Props) {
  const [step, setStep] = useState(initialStep);
  const next = () => setStep((s) => s + 1);

  return (
    <div className="setup-wizard">
      {step === 0 && <StepWelcome onNext={next} />}
      {step === 1 && <StepFastembed onNext={next} />}
      {step === 2 && <StepModel onNext={next} />}
      {step === 3 && <StepDone onComplete={onComplete} />}
    </div>
  );
}
