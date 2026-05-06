import { useState } from "react";
import { StepWelcome } from "./StepWelcome";
import { StepOllama } from "./StepOllama";
import { StepVaultPicker } from "./StepVaultPicker";
import { StepDone } from "./StepDone";

interface Props {
  onComplete: (vaultPath: string) => void;
  initialStep?: number;
}

export function SetupWizard({ onComplete, initialStep = 0 }: Props) {
  const [step, setStep] = useState(initialStep);
  const [vaultPath, setVaultPath] = useState<string>("");
  const next = () => setStep((s) => s + 1);

  return (
    <div className="setup-wizard">
      {step === 0 && <StepWelcome onNext={next} />}
      {step === 1 && <StepOllama onNext={next} />}
      {step === 2 && (
        <StepVaultPicker
          onNext={(path) => { setVaultPath(path); next(); }}
        />
      )}
      {step === 3 && <StepDone onComplete={() => onComplete(vaultPath)} />}
    </div>
  );
}
