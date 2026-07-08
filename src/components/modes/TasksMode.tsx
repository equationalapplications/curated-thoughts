import type { NavTarget } from "../../lib/navigation";

export interface TasksModeProps {
  onNavigate: (target: NavTarget) => void;
}

export function TasksMode({ onNavigate }: TasksModeProps) {
  return <div>Tasks Mode (placeholder)</div>;
}
