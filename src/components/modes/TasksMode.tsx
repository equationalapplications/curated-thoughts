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

  // Group tasks by entity_name
  const groupedTasks = tasks.reduce(
    (acc, task) => {
      if (!acc[task.entity_name]) {
        acc[task.entity_name] = [];
      }
      acc[task.entity_name].push(task);
      return acc;
    },
    {} as Record<string, TaskRow[]>,
  );

  // Sort entity names alphabetically
  const sortedEntityNames = Object.keys(groupedTasks).sort();

  // Format date (relative or absolute)
  const formatDate = (timestamp: number) => {
    const date = new Date(timestamp * 1000);
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
      <aside className="sidebar">
        {/* Status filter */}
        <div>
          <label
            style={{
              fontSize: "11px",
              fontWeight: 600,
              display: "block",
              marginBottom: "8px",
            }}
          >
            Status
          </label>
          <div style={{ display: "flex", flexDirection: "column", gap: "4px" }}>
            <label
              style={{
                display: "flex",
                alignItems: "center",
                gap: "6px",
                cursor: "pointer",
              }}
            >
              <input
                type="radio"
                name="status"
                value="pending"
                checked={status === "pending"}
                onChange={() => setStatus("pending")}
              />
              Open
            </label>
            <label
              style={{
                display: "flex",
                alignItems: "center",
                gap: "6px",
                cursor: "pointer",
              }}
            >
              <input
                type="radio"
                name="status"
                value="done"
                checked={status === "done"}
                onChange={() => setStatus("done")}
              />
              Done
            </label>
            <label
              style={{
                display: "flex",
                alignItems: "center",
                gap: "6px",
                cursor: "pointer",
              }}
            >
              <input
                type="radio"
                name="status"
                value="archived"
                checked={status === "archived"}
                onChange={() => setStatus("archived")}
              />
              Archived
            </label>
          </div>
        </div>

        {/* Create new task form */}
        <div>
          <label
            style={{
              fontSize: "11px",
              fontWeight: 600,
              display: "block",
              marginBottom: "8px",
            }}
          >
            + New Task
          </label>
          {createError && (
            <p
              style={{
                fontSize: "12px",
                color: "var(--error)",
                marginBottom: "8px",
              }}
            >
              {createError}
            </p>
          )}
          <form
            onSubmit={handleCreateTask}
            style={{ display: "flex", flexDirection: "column", gap: "6px" }}
          >
            <select
              value={selectedEntityId}
              onChange={(e) => setSelectedEntityId(e.target.value)}
              style={{
                padding: "6px",
                borderRadius: "4px",
                border: "1px solid var(--outline-var)",
                fontSize: "13px",
                backgroundColor: "var(--elev-2)",
                color: "var(--on-surface)",
              }}
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
              style={{
                padding: "6px",
                borderRadius: "4px",
                border: "1px solid var(--outline-var)",
                fontSize: "13px",
                backgroundColor: "var(--elev-2)",
                color: "var(--on-surface)",
              }}
            />
            <button
              type="submit"
              disabled={
                createLoading ||
                !selectedEntityId ||
                !taskDescription.trim()
              }
              style={{
                padding: "6px 12px",
                backgroundColor: "var(--primary)",
                color: "var(--on-primary)",
                border: "none",
                borderRadius: "4px",
                fontSize: "13px",
                fontWeight: 600,
                cursor: createLoading ? "not-allowed" : "pointer",
                opacity:
                  createLoading ||
                  !selectedEntityId ||
                  !taskDescription.trim()
                    ? 0.5
                    : 1,
              }}
            >
              Create
            </button>
          </form>
        </div>
      </aside>

      <main
        style={{
          flex: 1,
          overflowY: "auto",
          padding: "16px 24px",
        }}
      >
        {error && (
          <p
            style={{
              color: "var(--error)",
              fontSize: "13px",
              marginBottom: "16px",
            }}
            role="alert"
          >
            {error}
          </p>
        )}

        {loading ? (
          <p
            style={{
              color: "var(--outline)",
              fontSize: "13px",
              fontStyle: "italic",
            }}
          >
            Loading tasks…
          </p>
        ) : tasks.length === 0 ? (
          <div style={{ color: "var(--outline)", fontSize: "13px" }}>
            <p style={{ fontStyle: "italic", marginBottom: "8px" }}>
              No {status === "archived" ? "archived" : "open"} tasks.
            </p>
            {status === "pending" && (
              <p style={{ fontSize: "12px", lineHeight: 1.5 }}>
                The librarian proposes tasks through Review; approve one or
                create your own.
              </p>
            )}
          </div>
        ) : (
          <div
            style={{
              display: "flex",
              flexDirection: "column",
              gap: "20px",
            }}
          >
            {sortedEntityNames.map((entityName) => (
              <div key={entityName}>
                <button
                  onClick={() => {
                    const entity = entities.find(
                      (e) => e.name === entityName,
                    );
                    if (entity) {
                      onNavigate({
                        mode: "brain",
                        entityId: entity.id,
                      });
                    }
                  }}
                  style={{
                    background: "none",
                    border: "none",
                    cursor: "pointer",
                    textDecoration: "underline",
                    fontSize: "14px",
                    fontWeight: 600,
                    color: "var(--primary)",
                    padding: 0,
                    marginBottom: "8px",
                  }}
                >
                  {entityName}
                </button>

                <div style={{ display: "flex", flexDirection: "column", gap: "6px" }}>
                  {groupedTasks[entityName]
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
                        style={{
                          display: "flex",
                          gap: "8px",
                          alignItems: "flex-start",
                          padding: "8px",
                          backgroundColor: "var(--elev-2)",
                          borderRadius: "6px",
                        }}
                      >
                        <input
                          type="checkbox"
                          checked={task.status === "done"}
                          onChange={() =>
                            handleToggleTaskStatus(task.id, task.status)
                          }
                          style={{ marginTop: "2px", cursor: "pointer" }}
                          aria-label={`Mark "${task.description}" as ${task.status === "done" ? "pending" : "done"}`}
                        />
                        <div style={{ flex: 1, minWidth: 0 }}>
                          <p
                            style={{
                              margin: 0,
                              fontSize: "13px",
                              color:
                                task.status === "done"
                                  ? "var(--outline)"
                                  : "var(--on-surface)",
                              textDecoration:
                                task.status === "done"
                                  ? "line-through"
                                  : "none",
                            }}
                          >
                            {task.description}
                          </p>
                          <p
                            style={{
                              margin: "2px 0 0",
                              fontSize: "11px",
                              color: "var(--outline)",
                            }}
                          >
                            {formatDate(task.created_at)}
                          </p>
                        </div>
                        <button
                          onClick={() => handleArchiveTask(task.id)}
                          style={{
                            background: "none",
                            border: "none",
                            cursor: "pointer",
                            padding: "0 4px",
                            fontSize: "16px",
                            color: "var(--outline)",
                            opacity: 0.6,
                          }}
                          aria-label={`Archive task "${task.description}"`}
                        >
                          ×
                        </button>
                      </div>
                    ))}
                </div>
              </div>
            ))}
          </div>
        )}
      </main>
    </div>
  );
}
