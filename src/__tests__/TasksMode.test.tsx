import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { TasksMode } from "../components/modes/TasksMode";

vi.mock("@tauri-apps/api/core");

const ENTITIES = [
  {
    id: "entity-1",
    name: "Alice",
    entity_type: "Person",
    summary_snippet: "A person",
    fact_count: 1,
    open_task_count: 1,
    created_at: 1000,
    updated_at: 2000,
  },
  {
    id: "entity-2",
    name: "Bob",
    entity_type: "Person",
    summary_snippet: "Another person",
    fact_count: 2,
    open_task_count: 2,
    created_at: 1500,
    updated_at: 2500,
  },
];

const OPEN_TASKS = [
  {
    id: "task-1",
    entity_id: "entity-1",
    entity_name: "Alice",
    description: "Review documents",
    status: "pending",
    priority: 1,
    created_at: 1600,
  },
  {
    id: "task-2",
    entity_id: "entity-2",
    entity_name: "Bob",
    description: "Schedule meeting",
    status: "pending",
    priority: 2,
    created_at: 1700,
  },
  {
    id: "task-3",
    entity_id: "entity-1",
    entity_name: "Alice",
    description: "Send email",
    status: "pending",
    priority: 1,
    created_at: 1800,
  },
];

const DONE_TASKS = [
  {
    id: "task-4",
    entity_id: "entity-1",
    entity_name: "Alice",
    description: "Review documents",
    status: "done",
    priority: 1,
    created_at: 1600,
  },
];

const ARCHIVED_TASKS = [
  {
    id: "task-5",
    entity_id: "entity-2",
    entity_name: "Bob",
    description: "Old archived task",
    status: "pending",
    priority: 0,
    created_at: 1000,
  },
];

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation((cmd: string, args?: Record<string, unknown>) => {
    if (cmd === "list_entities_cmd") {
      return Promise.resolve(ENTITIES);
    }
    if (cmd === "list_tasks_cmd") {
      const status = args?.status as string | undefined;
      const includeArchived = args?.includeArchived as boolean | undefined;

      if (includeArchived === true) {
        return Promise.resolve(ARCHIVED_TASKS);
      } else if (status === "done") {
        return Promise.resolve(DONE_TASKS);
      } else {
        return Promise.resolve(OPEN_TASKS);
      }
    }
    if (cmd === "create_task_cmd") {
      const entityId = args?.entityId as string;
      const description = args?.description as string;
      return Promise.resolve({
        id: "task-new",
        entity_id: entityId,
        entity_name: ENTITIES.find((e) => e.id === entityId)?.name || "Unknown",
        description,
        status: "pending",
        priority: 0,
        created_at: Date.now() / 1000,
      });
    }
    if (cmd === "set_task_status_cmd") {
      return Promise.resolve(undefined);
    }
    if (cmd === "archive_task_cmd") {
      return Promise.resolve(undefined);
    }
    return Promise.resolve(null);
  });
});

test("groups_tasks_under_entity_headings", async () => {
  const onNavigate = vi.fn();
  render(<TasksMode onNavigate={onNavigate} />);

  // Should show Alice and Bob as entity headings
  const aliceHeading = await screen.findByRole("button", { name: "Alice" });
  const bobHeading = await screen.findByRole("button", { name: "Bob" });

  expect(aliceHeading).toBeInTheDocument();
  expect(bobHeading).toBeInTheDocument();

  // Verify Alice has 2 tasks and Bob has 1 task
  expect(screen.getByText("Review documents")).toBeInTheDocument();
  expect(screen.getByText("Schedule meeting")).toBeInTheDocument();
  expect(screen.getByText("Send email")).toBeInTheDocument();
});

test("status_filter_switches_listTasks_call", async () => {
  const onNavigate = vi.fn();
  render(<TasksMode onNavigate={onNavigate} />);

  // Initially shows "Open" tasks
  await screen.findByText("Review documents");

  // Change to Done filter
  const doneRadio = screen.getByLabelText("Done");
  fireEvent.click(doneRadio);

  // Should now show done tasks
  await waitFor(() => {
    expect(vi.mocked(invoke)).toHaveBeenCalledWith(
      "list_tasks_cmd",
      expect.objectContaining({ status: "done", includeArchived: false }),
    );
  });

  // Change to Archived filter
  const archivedRadio = screen.getByLabelText("Archived");
  fireEvent.click(archivedRadio);

  await waitFor(() => {
    expect(vi.mocked(invoke)).toHaveBeenCalledWith(
      "list_tasks_cmd",
      expect.objectContaining({ includeArchived: true }),
    );
  });
});

test("checking_task_calls_setTaskStatus_and_refreshes", async () => {
  const onNavigate = vi.fn();
  render(<TasksMode onNavigate={onNavigate} />);

  // Find the checkbox for a task
  const checkboxes = await screen.findAllByRole("checkbox");
  const firstCheckbox = checkboxes[0];

  expect(firstCheckbox).not.toBeChecked();

  fireEvent.click(firstCheckbox);

  // Should call setTaskStatus with 'done'
  await waitFor(() => {
    expect(vi.mocked(invoke)).toHaveBeenCalledWith(
      "set_task_status_cmd",
      expect.objectContaining({ status: "done" }),
    );
  });

  // Should refresh tasks after status change
  await waitFor(() => {
    expect(vi.mocked(invoke)).toHaveBeenCalledWith(
      "list_tasks_cmd",
      expect.any(Object),
    );
  });
});

test("entity_heading_navigates_to_brain", async () => {
  const onNavigate = vi.fn();
  render(<TasksMode onNavigate={onNavigate} />);

  const aliceHeading = await screen.findByRole("button", { name: "Alice" });
  fireEvent.click(aliceHeading);

  expect(onNavigate).toHaveBeenCalledWith({
    mode: "brain",
    entityId: "entity-1",
  });
});

test("create_form_calls_createTask", async () => {
  const onNavigate = vi.fn();
  render(<TasksMode onNavigate={onNavigate} />);

  // Wait for entities to load
  await screen.findByText("Select entity…");

  // Select entity from dropdown
  const entitySelect = screen.getByDisplayValue("Select entity…");
  fireEvent.change(entitySelect, { target: { value: "entity-1" } });

  // Type description
  const descriptionInput = screen.getByPlaceholderText("Description");
  fireEvent.change(descriptionInput, {
    target: { value: "New task description" },
  });

  // Submit form
  const createButton = screen.getByRole("button", { name: "Create" });
  fireEvent.click(createButton);

  // Should call createTask
  await waitFor(() => {
    expect(vi.mocked(invoke)).toHaveBeenCalledWith(
      "create_task_cmd",
      expect.objectContaining({
        entityId: "entity-1",
        description: "New task description",
      }),
    );
  });

  // Should clear form and refresh
  await waitFor(() => {
    expect(descriptionInput).toHaveValue("");
  });
});
