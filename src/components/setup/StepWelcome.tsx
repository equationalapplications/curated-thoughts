interface Props { onNext: () => void }

export function StepWelcome({ onNext }: Props) {
  return (
    <div className="setup-step">
      <h1>Your Second Brain</h1>
      <p>Private by default. Your documents never leave your machine.</p>
      <button onClick={onNext}>Get Started</button>
    </div>
  );
}
