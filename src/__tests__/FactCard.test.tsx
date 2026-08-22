import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { FactCard } from "../components/brain/FactCard";
import { type EntityFact } from "../lib/tauri";
import { vi } from "vitest";

vi.mock("@tauri-apps/api/core");

const FACT: EntityFact = {
  id: "fact_1",
  title: "Ships on Fridays",
  body: "Ships on Fridays with [[Beta Team]].",
  tags: [],
  confidence: "confirmed",
  source_type: "user_stated",
  source_docs: [{ path: "documents/notes.md", chunkId: null }],
  updated_at: 1750000000000,
};

test("renders body with wikilink chip, meta chips, and source chip", () => {
  const onNavigateEntity = vi.fn();
  const onOpenSource = vi.fn();
  render(
    <FactCard
      entityId="ent_1"
      fact={FACT}
      onChanged={vi.fn()}
      onNavigateEntity={onNavigateEntity}
      onOpenSource={onOpenSource}
    />,
  );
  expect(screen.getByText("confirmed")).toBeInTheDocument();
  expect(screen.getByText("user_stated")).toBeInTheDocument();

  fireEvent.click(screen.getByRole("button", { name: "Beta Team" }));
  expect(onNavigateEntity).toHaveBeenCalledWith("Beta Team");

  fireEvent.click(screen.getByRole("button", { name: "notes.md" }));
  // chunkId is null here because this fixture's source_docs entry has none.
  expect(onOpenSource).toHaveBeenCalledWith("documents/notes.md", null);
});

test("source chip passes the enriched chunkId to onOpenSource", () => {
  const onOpenSource = vi.fn();
  const fact: EntityFact = {
    ...FACT,
    id: "fact_chunked",
    source_docs: [{ path: "documents/notes.md", chunkId: "0123456789abcdef0123456789abcdef" }],
  };
  render(
    <FactCard
      entityId="ent_1"
      fact={fact}
      onChanged={vi.fn()}
      onNavigateEntity={vi.fn()}
      onOpenSource={onOpenSource}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: "notes.md" }));
  expect(onOpenSource).toHaveBeenCalledWith("documents/notes.md", "0123456789abcdef0123456789abcdef");
});

test("inline edit saves via update_entity_fact_cmd and notifies", async () => {
  const onChanged = vi.fn();
  render(
    <FactCard
      entityId="ent_1"
      fact={FACT}
      onChanged={onChanged}
      onNavigateEntity={vi.fn()}
      onOpenSource={vi.fn()}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Edit" }));
  fireEvent.change(screen.getByLabelText("Fact body"), {
    target: { value: "Ships on Mondays." },
  });
  fireEvent.click(screen.getByRole("button", { name: "Save" }));

  await waitFor(() => expect(onChanged).toHaveBeenCalled());
  expect(invoke).toHaveBeenCalledWith("update_entity_fact_cmd", {
    entityId: "ent_1",
    factId: "fact_1",
    body: "Ships on Mondays.",
  });
});

test("archive calls archive_entity_fact_cmd and notifies", async () => {
  const onChanged = vi.fn();
  render(
    <FactCard
      entityId="ent_1"
      fact={FACT}
      onChanged={onChanged}
      onNavigateEntity={vi.fn()}
      onOpenSource={vi.fn()}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Archive" }));
  await waitFor(() => expect(onChanged).toHaveBeenCalled());
  expect(invoke).toHaveBeenCalledWith("archive_entity_fact_cmd", {
    entityId: "ent_1",
    factId: "fact_1",
  });
});

test("alt+click invokes onPeekSource, not onOpenSource", () => {
  const onOpenSource = vi.fn();
  const onPeekSource = vi.fn();
  const fact: EntityFact = {
    ...FACT,
    id: "fact_peek",
    source_docs: [{ path: "documents/notes.md", chunkId: "abc123" }],
  };
  render(
    <FactCard
      entityId="ent_1"
      fact={fact}
      onChanged={vi.fn()}
      onNavigateEntity={vi.fn()}
      onOpenSource={onOpenSource}
      onPeekSource={onPeekSource}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: "notes.md" }), { altKey: true });
  expect(onPeekSource).toHaveBeenCalledWith("documents/notes.md", "abc123");
  expect(onOpenSource).not.toHaveBeenCalled();
});

test("plain click still invokes onOpenSource even with onPeekSource set", () => {
  const onOpenSource = vi.fn();
  const onPeekSource = vi.fn();
  const fact: EntityFact = {
    ...FACT,
    id: "fact_plain",
    source_docs: [{ path: "documents/notes.md", chunkId: "abc123" }],
  };
  render(
    <FactCard
      entityId="ent_1"
      fact={fact}
      onChanged={vi.fn()}
      onNavigateEntity={vi.fn()}
      onOpenSource={onOpenSource}
      onPeekSource={onPeekSource}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: "notes.md" }));
  expect(onOpenSource).toHaveBeenCalledWith("documents/notes.md", "abc123");
  expect(onPeekSource).not.toHaveBeenCalled();
});

test("alt+click with chunkId null falls back to onOpenSource", () => {
  const onOpenSource = vi.fn();
  const onPeekSource = vi.fn();
  render(
    <FactCard
      entityId="ent_1"
      fact={FACT}
      onChanged={vi.fn()}
      onNavigateEntity={vi.fn()}
      onOpenSource={onOpenSource}
      onPeekSource={onPeekSource}
    />,
  );
  // The FACT fixture's source_docs entry has chunkId: null.
  fireEvent.click(screen.getByRole("button", { name: "notes.md" }), { altKey: true });
  expect(onOpenSource).toHaveBeenCalledWith("documents/notes.md", null);
  expect(onPeekSource).not.toHaveBeenCalled();
});

test("alt+click without onPeekSource prop behaves as plain click", () => {
  const onOpenSource = vi.fn();
  const fact: EntityFact = {
    ...FACT,
    id: "fact_nopeek",
    source_docs: [{ path: "documents/notes.md", chunkId: "abc123" }],
  };
  render(
    <FactCard
      entityId="ent_1"
      fact={fact}
      onChanged={vi.fn()}
      onNavigateEntity={vi.fn()}
      onOpenSource={onOpenSource}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: "notes.md" }), { altKey: true });
  expect(onOpenSource).toHaveBeenCalledWith("documents/notes.md", "abc123");
});
