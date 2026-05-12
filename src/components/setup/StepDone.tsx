interface Props {
  onComplete: () => void | Promise<void>;
}

export function StepDone({ onComplete }: Props) {
  return (
    <div className="setup-step">
      <h2>You're all set!</h2>
      <p>Your librarian is ready to start curating your thoughts.</p>
      <button onClick={onComplete}>Open My Brain</button>
    </div>
  );
}
