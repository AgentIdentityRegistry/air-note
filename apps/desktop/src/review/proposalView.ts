import type { ProposalDto } from "../api/engine";

/** A display row for one queued proposal (pure: path split, op label, risk flag, source label). */
export type ProposalRow = {
  id: string;
  fileName: string;
  folder: string;
  why: string;
  risky: boolean;
  opLabel: string;
  fromMandate: boolean;
};

const OP_LABEL: Record<string, string> = { edit: "Edit", create: "Create", delete: "Delete" };
/** The engine's M6c mandate-proposer producer stamp (graph.rs M6C_PROPOSER_PRODUCER). */
const M6C_PRODUCER = "m6c-mandate-proposer";

/** Map a proposal DTO to a display row. `risky` mirrors the propose-time loud-modal flag;
 *  `fromMandate` is true for an M6c mandate-driven rewrite (so the queue can label it). */
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
    fromMandate: p.producer === M6C_PRODUCER,
  };
}
