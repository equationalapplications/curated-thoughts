import type { HealthState } from "../../hooks/useProviderHealth";

type Feature =
  | "search"
  | "synthesis"
  | "similarity"
  | "indexing"
  | "related_notes";

interface Props {
  feature: Feature;
  embedding: HealthState;
  generation: HealthState;
}

/**
 * Returns the spec §7 inline notice message for a (feature, embedding, generation)
 * triple, or `null` when no notice is warranted.
 *
 * Mapping rules (spec §7):
 *   - features that depend on the embedder: "search", "similarity", "indexing",
 *     "related_notes" show "<feature> needs the embedder — check Models" when
 *     embedding === "error".
 *   - features that depend on a generation backend: "synthesis"
 *     shows "<feature> needs a generation backend — check Models" when
 *     generation === "unconfigured".
 *   - any other state: no notice.
 *
 * "loading" never trips the notice (transient state), and "ok" / "unconfigured"
 * on the wrong provider are also no-ops (synthesis doesn't need an embedder, etc.).
 */
function noticeFor(
  feature: Feature,
  embedding: HealthState,
  generation: HealthState,
): string | null {
  const dependsOnEmbedder =
    feature === "search" ||
    feature === "similarity" ||
    feature === "indexing" ||
    feature === "related_notes";
  const dependsOnGeneration = feature === "synthesis";

  if (dependsOnEmbedder && embedding === "error") {
    if (feature === "related_notes") {
      return "Related notes need the embedder — check Models";
    }
    return `${feature} needs the embedder — check Models`;
  }
  if (dependsOnGeneration && generation === "unconfigured") {
    return `${feature} needs a generation backend — check Models`;
  }
  return null;
}

/**
 * Inline notice rendered above surfaces whose feature is degraded by an
 * embedder or generation backend that is down or unconfigured. Returns
 * `null` when the relevant provider is healthy for the given feature.
 *
 * Per spec §7: the app never hard-blocks reading when a provider is down;
 * it surfaces a quiet inline notice instead.
 */
export function ProviderNotice({ feature, embedding, generation }: Props) {
  const message = noticeFor(feature, embedding, generation);
  if (!message) return null;
  return (
    <div className="provider-notice" role="status" aria-live="polite">
      <span className="provider-notice-dot" aria-hidden="true" />
      <span className="provider-notice-message">{message}</span>
    </div>
  );
}