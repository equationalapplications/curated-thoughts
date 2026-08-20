import { WizardStep } from "./WizardStep";

interface Props {
  onComplete: () => void | Promise<void>;
}

export function StepDone({ onComplete }: Props) {
  return (
    <WizardStep
      title="You're ready"
      subtitle="Your librarian is ready to start curating your thoughts."
      onNext={onComplete}
      nextLabel="Open My Brain"
    >
      <p>Welcome to Curated Thoughts.</p>
    </WizardStep>
  );
}
