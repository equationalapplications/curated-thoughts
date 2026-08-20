import { WizardStep } from "./WizardStep";

interface Props { onNext: () => void; vaultPath?: string | null }

export function StepWelcome({ onNext, vaultPath }: Props) {
  return (
    <WizardStep
      title="Where is your vault?"
      subtitle="Read-only: the folder your notes live in."
      onNext={onNext}
    >
      {vaultPath ? <p>{vaultPath}</p> : <p>Your vault path will appear here once selected.</p>}
    </WizardStep>
  );
}
