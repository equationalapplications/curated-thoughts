import { useEffect, useMemo, useRef, useState } from "react";
import {
  listEntities,
  listVaultFiles,
  type EntitySummary,
  type VaultFile,
} from "../../lib/tauri";
import {
  commandNavigate,
  useCommands,
  type Command,
  type CommandScope,
} from "../../lib/commands";

/** Dynamic results are capped post-filter to bound the DOM on large vaults. */
const MAX_DYNAMIC_RESULTS = 8;

interface Props {
  /** Scope the palette was opened from — `mode:${nav.current.mode}`. */
  scope: CommandScope;
  onClose: () => void;
}

export function CommandPalette({ scope, onClose }: Props) {
  const commands = useCommands(scope);
  const [query, setQuery] = useState("");
  const [entities, setEntities] = useState<EntitySummary[]>([]);
  const [files, setFiles] = useState<VaultFile[]>([]);
  const [activeIndex, setActiveIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  // Focus contract (mirrors PeekPanel): remember what had focus before the
  // palette opened, put focus on the input, and give it back on unmount.
  // Every close path — Esc, backdrop, dispatch — funnels through onClose →
  // unmount, so this single cleanup covers them all.
  useEffect(() => {
    const opener = document.activeElement as HTMLElement | null;
    inputRef.current?.focus();
    return () => opener?.focus();
  }, []);

  // Client-side search over already-available data — the existing
  // listEntities / listVaultFiles commands. No new IPC surface.
  useEffect(() => {
    let cancelled = false;
    listEntities("name_asc")
      .then((rows) => {
        if (!cancelled) setEntities(rows);
      })
      .catch(() => {
        if (!cancelled) setEntities([]);
      });
    listVaultFiles()
      .then((rows) => {
        if (!cancelled) setFiles(rows);
      })
      .catch(() => {
        if (!cancelled) setFiles([]);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const visible: Command[] = useMemo(() => {
    const q = query.trim().toLowerCase();
    const scoped = commands.filter((c) => !q || c.label.toLowerCase().includes(q));
    const dynamic: Command[] = [];
    if (q) {
      const matchedEntities = entities.filter((entity) =>
        entity.name.toLowerCase().includes(q),
      );
      const matchedFiles = files.filter(
        (file) => file.tier === "user_doc" && file.name.toLowerCase().includes(q),
      );
      for (const entity of matchedEntities.slice(0, MAX_DYNAMIC_RESULTS)) {
        dynamic.push({
          id: `entity:${entity.id}`,
          label: `Open entity: ${entity.name}`,
          scope: "global",
          run: () => commandNavigate({ mode: "brain", entityId: entity.id }),
        });
      }
      for (const file of matchedFiles.slice(0, MAX_DYNAMIC_RESULTS)) {
        dynamic.push({
          id: `document:${file.path}`,
          label: `Open document: ${file.name}`,
          scope: "global",
          run: () => commandNavigate({ mode: "library", docPath: file.path }),
        });
      }
    }
    return [...scoped, ...dynamic];
  }, [commands, entities, files, query]);

  const index = visible.length === 0 ? -1 : Math.min(activeIndex, visible.length - 1);

  function dispatch(cmd: Command | undefined) {
    if (!cmd) return;
    cmd.run();
    onClose();
  }

  function handleInputKeyDown(e: React.KeyboardEvent) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActiveIndex((i) => Math.min(i + 1, Math.max(visible.length - 1, 0)));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActiveIndex((i) => Math.max(i - 1, 0));
    } else if (e.key === "Tab") {
      // The input is the panel's only focusable stop while open; pinning Tab
      // keeps the aria-modal promise truthful without a full focus trap.
      e.preventDefault();
    } else if (e.key === "Enter") {
      // Confirming an IME candidate (CJK input) must not dispatch
      // mid-composition.
      if (e.nativeEvent.isComposing) return;
      e.preventDefault();
      dispatch(visible[index]);
    }
  }

  // Esc closes the palette before any Esc semantics inside the focused
  // control: capture phase + stopPropagation beats bubble-phase listeners.
  useEffect(() => {
    function onWindowKeyDown(e: KeyboardEvent) {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopPropagation();
      onClose();
    }
    window.addEventListener("keydown", onWindowKeyDown, true);
    return () => window.removeEventListener("keydown", onWindowKeyDown, true);
  }, [onClose]);

  return (
    <>
      <button
        type="button"
        className="palette-backdrop"
        aria-label="Close command palette"
        onClick={onClose}
      />
      <div
        className="palette-panel"
        role="dialog"
        aria-modal="true"
        aria-label="Command palette"
      >
        <input
          ref={inputRef}
          className="palette-input"
          type="text"
          placeholder="Type a command…"
          aria-label="Search commands"
          role="combobox"
          aria-expanded="true"
          aria-controls="palette-listbox"
          aria-activedescendant={index >= 0 ? `palette-option-${index}` : undefined}
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setActiveIndex(0);
          }}
          onKeyDown={handleInputKeyDown}
        />
        <ul id="palette-listbox" role="listbox" aria-label="Commands" className="palette-list">
          {visible.length === 0 && (
            <li className="palette-empty">No matching commands.</li>
          )}
          {visible.map((cmd, i) => (
            <li
              key={cmd.id}
              id={`palette-option-${i}`}
              role="option"
              aria-selected={i === index}
              className={`palette-option${i === index ? " palette-option--active" : ""}`}
              onMouseEnter={() => setActiveIndex(i)}
              onClick={() => dispatch(cmd)}
            >
              {cmd.label}
            </li>
          ))}
        </ul>
      </div>
    </>
  );
}
