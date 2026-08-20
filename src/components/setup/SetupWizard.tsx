import { useState } from "react";
import { StepWelcome } from "./StepWelcome";
import { StepPrivacy } from "./StepPrivacy";
import { StepFastembed } from "./StepFastembed";
import { StepModel } from "./StepModel";
import { StepDone } from "./StepDone";
import { StepWatchItThink } from "./StepWatchItThink";
import { StepIndicator } from "./StepIndicator";

const STEPS = ["Welcome", "Privacy", "Fastembed", "Model", "Watch it think", "Done"];

interface Props {
  onComplete: () => void | Promise<void>;
  initialStep?: number;
  vaultPath?: string | null;
  onRouteToReview?: (proposalId: string | null) => void;
}

export function SetupWizard({ onComplete, initialStep = 0, vaultPath, onRouteToReview }: Props) {
  const [step, setStep] = useState(initialStep);
  const next = () => setStep((s) => s + 1);
  return (
    <div className="setup-wizard">
      <StepIndicator current={step} total={STEPS.length} steps={STEPS} />
      {step === 0 && <StepWelcome onNext={next} vaultPath={vaultPath} />}
      {step === 1 && <StepPrivacy onNext={next} />}
      {step === 2 && <StepFastembed onNext={next} />}
      {step === 3 && <StepModel onNext={next} />}
      {step === 4 && <StepWatchItThink onSkip={next} onRouteToReview={onRouteToReview ?? (() => {})} />}
      {step === 5 && <StepDone onComplete={onComplete} />}
    </div>
  );
}
