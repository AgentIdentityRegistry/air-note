import type { GrantDto } from "../api/engine";

export const activeGrants = (all: GrantDto[]): GrantDto[] => all.filter((g) => !g.revoked);
