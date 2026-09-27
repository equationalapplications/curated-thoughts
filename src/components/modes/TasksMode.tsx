import { useCallback, useEffect, useState } from "react";
import {
  listTasks,
  createTask,
  setTaskStatus,
  archiveTask,
  listEntities,
  type TaskRow,
  type EntitySummary,
} from "../../lib/tauri";
import type { NavTarget } from "../../lib/navigation";

export interface TasksModeProps {
  onNavigate: (target: NavTarget) => void;
}

export function TasksMode({ onNavigate }: TasksModeProps) {
  const [status, setStatus] = useState<"pending" | "done" | "archived">(
    "pending",
  );
  const [tasks, setTasks] = useState<TaskRow[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [entities, setEntities] = useState<EntitySummary[]>([]);
  const [createError, setCreateError] = useState<string | null>(null);
  const [createLoading, setCreateLoading] = useState(false);
  const [selectedEntityId, setSelectedEntityId] = useState("");
  const [taskDescription, setTaskDescription] = useState("");

  // Load tasks when status changes
  useEffect(() => {
    (async () => {
      setLoading(true);
      setError(null);
      try {
        let taskStatus: "pending" | "done" | undefined;
        let includeArchived: boolean | undefined;

        if (status === "archived") {
          taskStatus = undefined;
          includeArchived = true;
        } else {
          taskStatus = status;
          includeArchived = false;
        }

        const loaded = await listTasks(taskStatus, includeArchived);
        setTasks(loaded);
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        setLoading(false);
      }
    })();
  }, [status]);

  // Load entities on mount
  useEffect(() => {
    (async () => {
      try {
        const loaded = await listEntities();
        setEntities(loaded);
      } catch {
        // Silent fail on entities load; form can still work
      }
    })();
  }, []);

  // Callback to refresh tasks (used after creating/updating tasks)
  const refreshTasks = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      let taskStatus: "pending" | "done" | undefined;
      let includeArchived: boolean | undefined;

      if (status === "archived") {
        taskStatus = undefined;
        includeArchived = true;
      } else {
        taskStatus = status;
        includeArchived = false;
      }

      const loaded = await listTasks(taskStatus, includeArchived);
      setTasks(loaded);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, [status]);

  const handleCreateTask = useCallback(
    async (e: React.FormEvent) => {
      e.preventDefault();
      if (!selectedEntityId || !taskDescription.trim()) return;

      setCreateError(null);
      setCreateLoading(true);
      try {
        await createTask(selectedEntityId, taskDescription.trim());
        setSelectedEntityId("");
        setTaskDescription("");
        await refreshTasks();
      } catch (err) {
        setCreateError(err instanceof Error ? err.message : String(err));
      } finally {
        setCreateLoading(false);
      }
    },
    [selectedEntityId, taskDescription, refreshTasks],
  );

  const handleToggleTaskStatus = useCallback(
    async (taskId: string, currentStatus: string) => {
      const newStatus = currentStatus === "pending" ? "done" : "pending";
      setError(null);
      try {
        await setTaskStatus(taskId, newStatus as "pending" | "done");
        await refreshTasks();
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      }
    },
    [refreshTasks],
  );

  const handleArchiveTask = useCallback(
    async (taskId: string) => {
      setError(null);
      try {
        await archiveTask(taskId);
        await refreshTasks();
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      }
    },
    [refreshTasks],
  );

  // Group tasks by entity_id (curated_entities.name is not unique, so id is the
  // only safe key). entity_name is kept only for display and sorting.
  const groupedTasks = tasks.reduce(
    (acc, task) => {
      if (!acc[task.entity_id]) {
        acc[task.entity_id] = [];
      }
      acc[task.entity_id].push(task);
      return acc;
    },
    {} as Record<string, TaskRow[]>,
  );

  // Sort groups by their display name alphabetically
  const sortedEntityIds = Object.keys(groupedTasks).sort((a, b) => {
    const aName = groupedTasks[a][0]?.entity_name ?? "";
    const bName = groupedTasks[b][0]?.entity_name ?? "";
    return aName.localeCompare(bName);
  });

  // Format date (relative or absolute). `created_at` arrives in milliseconds
  // from the backend (see db::tasks::create_task using now_timestamps().1),
  // so the Date constructor takes it as-is.
  const formatDate = (timestampMs: number) => {
    const date = new Date(timestampMs);
    const now = Date.now();
    const diffMs = now - date.getTime();
    const diffDays = Math.floor(diffMs / (1000 * 60 * 60 * 24));

    if (diffDays === 0) {
      return date.toLocaleTimeString(undefined, {
        hour: "numeric",
        minute: "2-digit",
      });
    } else if (diffDays === 1) {
      return "Yesterday";
    } else if (diffDays < 7) {
      return `${diffDays}d ago`;
    } else {
      return date.toLocaleDateString(undefined, {
        month: "short",
        day: "numeric",
      });
    }
  };

  return (
    <div className="mode-layout">
      <aside className="mode-sidebar">
        {/* Status filter */}
        <section>
          <h3>Status</h3>
          <div className="tasks-status-filter" role="radiogroup" aria-label="Task status">
            {(["pending", "done", "archived"] as const).map((value) => (
              <label key={value} className="tasks-status-option">
                <input
                  type="radio"
                  name="status"
                  value={value}
                  checked={status === value}
                  onChange={() => setStatus(value)}
                />
                {value === "pending" ? "Open" : value === "done" ? "Done" : "Archived"}
              </label>
            ))}
          </div>
        </section>

        {/* Create new task form */}
        <section>
          <h3>+ New Task</h3>
          {createError && <p className="tasks-form-error">{createError}</p>}
          <form className="tasks-create-form" onSubmit={handleCreateTask}>
            <select
              aria-label="Entity"
              value={selectedEntityId}
              onChange={(e) => setSelectedEntityId(e.target.value)}
            >
              <option value="">Select entity…</option>
              {entities.map((e) => (
                <option key={e.id} value={e.id}>
                  {e.name}
                </option>
              ))}
            </select>
            <input
              type="text"
              placeholder="Description"
              value={taskDescription}
              onChange={(e) => setTaskDescription(e.target.value)}
            />
            <button
              type="submit"
              className="btn btn--primary"
              disabled={
                createLoading ||
                !selectedEntityId ||
                !taskDescription.trim()
              }
            >
              Create
            </button>
          </form>
        </section>
      </aside>

      <main className="mode-main tasks-main">
        {error && (
          <p className="tasks-error" role="alert">
            {error}
          </p>
        )}

        {loading ? (
          <p className="placeholder tasks-loading">Loading tasks…</p>
        ) : tasks.length === 0 ? (
          /* Same .empty-pane block as Review and the editor. Tasks had its
             own `.tasks-empty`, which was a top-left-aligned pair of lines in
             a 1300px column while the Review equivalent sat centred — two
             sibling modes, two different answers to "nothing here yet". */
          <div className="empty-pane">
            <span className="empty-pane__icon" aria-hidden="true">
              <svg className="icon" viewBox="0 0 24 24" focusable="false">
                <path d="M9.5 6.5h9M9.5 12h9M9.5 17.5h9" />
                <path d="M4.6 6.6 5.5 8l2-2.4M4.6 12.1 5.5 13.5l2-2.4M4.6 17.6l.9 1.4 2-2.4" />
              </svg>
            </span>
            <h2 className="empty-pane__title">
              No{" "}
              {status === "archived"
                ? "archived"
                : status === "done"
                  ? "done"
                  : "open"}{" "}
              tasks.
            </h2>
            <p className="empty-pane__hint">
              {status === "pending"
                ? "The librarian proposes tasks through Review; approve one there, or create your own with New task."
                : "Switch the filter above to see tasks in another state."}
            </p>
          </div>
        ) : (
          <div className="tasks-groups">
            {sortedEntityIds.map((entityId) => {
              const group = groupedTasks[entityId];
              const entityName = group[0]?.entity_name ?? "";
              return (
                <section key={entityId} className="tasks-group">
                  <button
                    className="tasks-group-link"
                    onClick={() => {
                      onNavigate({
                        mode: "brain",
                        entityId,
                      });
                    }}
                  >
                    {entityName}
                  </button>
                  <div className="tasks-group-items">
                    {group
                      .sort((a, b) => {
                        // Sort by priority DESC, then by created_at ASC
                        if (a.priority !== b.priority) {
                          return b.priority - a.priority;
                        }
                        return a.created_at - b.created_at;
                      })
                      .map((task) => (
                        <div
                          key={task.id}
                          className={`tasks-row${
                            task.status === "done" ? " tasks-row--done" : ""
                          }`}
                        >
                          <input
                            type="checkbox"
                            className="tasks-row-checkbox"
                            checked={task.status === "done"}
                            onChange={() =>
                              handleToggleTaskStatus(task.id, task.status)
                            }
                            aria-label={`Mark "${task.description}" as ${task.status === "done" ? "pending" : "done"}`}
                          />
                          <div className="tasks-row-body">
                            <p className="tasks-row-content">
                              {task.description}
                            </p>
                            <p className="tasks-row-meta">
                              {formatDate(task.created_at)}
                            </p>
                          </div>
                          <button
                            className="icon-btn"
                            onClick={() => handleArchiveTask(task.id)}
                            aria-label={`Archive task "${task.description}"`}
                          >
                            <svg
                              className="icon icon--sm"
                              viewBox="0 0 24 24"
                              aria-hidden="true"
                              focusable="false"
                            >
                              <path d="M6 6l12 12M18 6 6 18" />
                            </svg>
                          </button>
                        </div>
                      ))}
                  </div>
                </section>
              );
            })}
          </div>
        )}
      </main>
    </div>
  );
}
