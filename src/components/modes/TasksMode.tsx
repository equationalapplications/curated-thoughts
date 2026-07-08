import { useState, useEffect, useCallback } from "react";
import {
  listTasks,
  createTask,
  resolveTask,
  archiveTask,
  listEntities,
  type TaskRow,
  type EntitySummary,
} from "../../lib/tauri";

type TaskStatus = "pending" | "done" | "archived";

export function TasksMode() {
  const [tasks, setTasks] = useState<TaskRow[]>([]);
  const [entities, setEntities] = useState<EntitySummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<TaskStatus>("pending");
  const [newDescription, setNewDescription] = useState("");
  const [newEntityId, setNewEntityId] = useState("");

  // Load tasks when status changes
  useEffect(() => {
    let ignore = false;
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
        if (!ignore) setTasks(loaded);
      } catch (err) {
        if (!ignore) setError(err instanceof Error ? err.message : String(err));
      } finally {
        if (!ignore) setLoading(false);
      }
    })();
    return () => {
      ignore = true;
    };
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

  const handleCreate = async () => {
    if (!newDescription.trim() || !newEntityId) return;
    try {
      await createTask(newEntityId, newDescription.trim());
      setNewDescription("");
      await refreshTasks();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const handleResolve = async (taskId: string) => {
    try {
      await resolveTask(taskId);
      await refreshTasks();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const handleArchive = async (taskId: string) => {
    try {
      await archiveTask(taskId);
      await refreshTasks();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  if (loading && tasks.length === 0) {
    return <div className="loading-screen">Loading tasks…</div>;
  }

  return (
    <div className="mode-layout">
      <div className="sidebar">
        <h2>Tasks</h2>
        <div className="search-bar">
          <input
            type="text"
            placeholder="Filter tasks…"
            // Filter logic omitted for brevity
          />
        </div>
        <div className="folder-tree">
          <button
            className={`tree-file ${status === "pending" ? "tree-file--active" : ""}`}
            onClick={() => setStatus("pending")}
          >
            Pending
          </button>
          <button
            className={`tree-file ${status === "done" ? "tree-file--active" : ""}`}
            onClick={() => setStatus("done")}
          >
            Done
          </button>
          <button
            className={`tree-file ${status === "archived" ? "tree-file--active" : ""}`}
            onClick={() => setStatus("archived")}
          >
            Archived
          </button>
        </div>
      </div>
      <main className="editor-pane editor-pane--active">
        {error && <p className="editor-error">{error}</p>}
        <div className="review-proposal-section">
          <h3 className="review-proposal-section-title">
            {status === "pending"
              ? "Pending tasks"
              : status === "done"
                ? "Completed tasks"
                : "Archived tasks"}
          </h3>
          {tasks.length === 0 && (
            <p className="review-hint">No tasks in this view.</p>
          )}
          <div className="proposal-item-list">
            {tasks.map((task) => (
              <div key={task.id} className="proposal-item-row">
                <div className="proposal-item-body">
                  <span className="proposal-item-label">
                    {task.entity_id} — {task.status}
                  </span>
                  <p className="proposal-item-detail">{task.description}</p>
                </div>
                <div className="proposal-item-actions">
                  {task.status === "pending" && (
                    <button
                      className="proposal-item-btn proposal-item-btn--accept"
                      onClick={() => handleResolve(task.id)}
                    >
                      Done
                    </button>
                  )}
                  <button
                    className="proposal-item-btn proposal-item-btn--reject"
                    onClick={() => handleArchive(task.id)}
                  >
                    Archive
                  </button>
                </div>
              </div>
            ))}
          </div>
        </div>
        <div className="review-proposal-section">
          <h3 className="review-proposal-section-title">New task</h3>
          <div className="rule-form">
            <select
              value={newEntityId}
              onChange={(e) => setNewEntityId(e.target.value)}
              className="rule-select"
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
              placeholder="Task description"
              value={newDescription}
              onChange={(e) => setNewDescription(e.target.value)}
              className="rule-input"
            />
            <button
              className="rule-add-btn"
              onClick={handleCreate}
              disabled={!newDescription.trim() || !newEntityId}
            >
              Create
            </button>
          </div>
        </div>
      </main>
    </div>
  );
}
