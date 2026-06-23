import type { ProposalDto } from "../api/engine";

/** A display row for one queued proposal (pure: path split, op label, risk flag). */
export type ProposalRow = {
  id: string;
  fileName: string;
  folder: string;
  why: string;
  risky: boolean;
  opLabel: string;
};

const OP_LABEL: Record<string, string> = { edit: "Edit", create: "Create", delete: "Delete" };

/** Map a proposal DTO to a display row. `risky` mirrors the propose-time loud-modal flag. */
export function toProposalRow(p: ProposalDto): ProposalRow {
  const slash = p.target.lastIndexOf("/");
  const fileName = slash >= 0 ? p.target.slice(slash + 1) : p.target;
  const folder = slash >= 0 ? p.target.slice(0, slash) : "";
  return {
    id: p.id,
    fileName,
    folder,
    why: p.rationale,
    risky: p.requires_loud_modal,
    opLabel: OP_LABEL[p.op] ?? p.op,
  };
}
