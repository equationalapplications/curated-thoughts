import { WizardStep } from "./WizardStep";
import { OntologyChoice } from "./OntologyChoice";

interface Props { onNext: () => void; vaultPath?: string | null }

export function StepWelcome({ onNext, vaultPath }: Props) {
  return (
    <WizardStep
      title="Where is your vault?"
      subtitle="Read-only: the folder your notes live in."
      onNext={onNext}
    >
      {vaultPath ? (
        /* Monospace, like the same path in Settings → Vault. As body text a
           filesystem path is hard to scan and the slashes stop reading as
           separators. */
        <p className="vault-full-path">{vaultPath}</p>
      ) : (
        <p>Your vault path will appear here once selected.</p>
      )}
      <OntologyChoice />
    </WizardStep>
  );
}
