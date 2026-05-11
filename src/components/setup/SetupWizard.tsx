import { useState } from "react";
import { StepWelcome } from "./StepWelcome";
import { StepOllama } from "./StepOllama";
import { StepDone } from "./StepDone";

interface Props {
  onComplete: () => void;
  initialStep?: number;
}

export function SetupWizard({ onComplete, initialStep = 0 }: Props) {
  const [step, setStep] = useState(initialStep);
  const next = () => setStep((s) => s + 1);

  return (
    <div className="setup-wizard">
      {step === 0 && <StepWelcome onNext={next} />}
      {step === 1 && <StepOllama onNext={next} />}
      {step === 2 && <StepDone onComplete={onComplete} />}
    </div>
  );
}
