import type { EntityDetail, ProposalDetail, ProposalItem } from "./tauri";

export function findSummaryUpdateItem(
  detail: ProposalDetail,
): ProposalItem | undefined {
  return detail.items.find((item) => item.item_type === "summary_update");
}

export function summaryTextFromItem(item: ProposalItem): string {
  const payload = item.edited_payload ?? item.payload;
  return typeof payload.summary === "string" ? payload.summary : "";
}

export function proposedSummaryText(detail: ProposalDetail): string {
  const summaryItem = findSummaryUpdateItem(detail);
  if (summaryItem) return summaryTextFromItem(summaryItem);
  return "";
}

export function nonSummaryItems(detail: ProposalDetail): ProposalItem[] {
  return detail.items.filter((item) => item.item_type !== "summary_update");
}

export function factBodyFromEntity(
  entity: EntityDetail | null | undefined,
  targetId: string | null | undefined,
): string | null {
  if (!entity || !targetId) return null;
  const fact = entity.facts.find((f) => f.id === targetId);
  return fact?.body ?? null;
}

export function itemPayloadString(
  item: ProposalItem,
  field: string,
): string {
  const payload = item.edited_payload ?? item.payload;
  const value = payload[field];
  return typeof value === "string" ? value : "";
}

export function describeProposalItem(
  item: ProposalItem,
  entity?: EntityDetail | null,
): { label: string; detail: string } {
  switch (item.item_type) {
    case "summary_update":
      return {
        label: "Summary update",
        detail: summaryTextFromItem(item) || "(empty summary)",
      };
    case "fact_add":
      return {
        label: "Add fact",
        detail: itemPayloadString(item, "body") || "(empty fact)",
      };
    case "fact_update": {
      const nextBody = itemPayloadString(item, "body");
      const previousBody = factBodyFromEntity(entity, item.target_id);
      if (previousBody) {
        return {
          label: "Update fact",
          detail: previousBody === nextBody ? nextBody : `${previousBody} → ${nextBody}`,
        };
      }
      return {
        label: "Update fact",
        detail: nextBody || "(empty fact)",
      };
    }
    case "fact_archive": {
      const archivedBody = factBodyFromEntity(entity, item.target_id);
      return {
        label: "Archive fact",
        detail: archivedBody ?? item.target_id ?? "unknown fact",
      };
    }
    case "edge_add":
      return {
        label: "Add edge",
        detail: itemPayloadString(item, "edge_type") || "related",
      };
    case "task_add":
      return {
        label: "Add task",
        detail: itemPayloadString(item, "description") || "(empty task)",
      };
    default:
      return { label: item.item_type, detail: "" };
  }
}
