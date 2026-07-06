import type { ProposalDetail } from "./tauri";

/** Mirrors Rust `format_proposal_preview` in `db/review_shim.rs`. */
export function formatProposalPreview(detail: ProposalDetail): string {
  let md = `# ${detail.target_name}\n\n`;
  if (detail.reasoning?.trim()) {
    md += `${detail.reasoning.trim()}\n`;
  }
  if (detail.items.length === 0) {
    md += "\n*No proposed items.*\n";
    return md;
  }

  md += "\n## Proposed changes\n\n";
  for (const item of detail.items) {
    switch (item.item_type) {
      case "summary_update": {
        const summary =
          typeof item.payload.summary === "string" ? item.payload.summary : "";
        md += "### Summary update\n\n";
        md += `${summary}\n\n`;
        break;
      }
      case "fact_add": {
        const body =
          typeof item.payload.body === "string" ? item.payload.body : "";
        md += `- **Add fact:** ${body}\n`;
        break;
      }
      case "fact_update": {
        const body =
          typeof item.payload.body === "string" ? item.payload.body : "";
        const tid = item.target_id ?? "?";
        md += `- **Update fact** \`${tid}\`: ${body}\n`;
        break;
      }
      case "fact_archive": {
        const tid = item.target_id ?? "?";
        md += `- **Archive fact** \`${tid}\`\n`;
        break;
      }
      case "edge_add": {
        const edgeType =
          typeof item.payload.edge_type === "string"
            ? item.payload.edge_type
            : "related";
        md += `- **Add edge:** ${edgeType}\n`;
        break;
      }
      case "task_add": {
        const desc =
          typeof item.payload.description === "string"
            ? item.payload.description
            : "";
        md += `- **Add task:** ${desc}\n`;
        break;
      }
      default:
        md += `- **${item.item_type}**\n`;
    }
    if (item.evidence.length > 0) {
      md += "  - Evidence:\n";
      for (const ev of item.evidence) {
        const quote = [...ev.quote].slice(0, 120).join("");
        md += `    - "${quote}"\n`;
      }
    }
  }
  return md;
}
