import type { VersionRef } from './execution-view'
export type BoardValidity = 'Current' | 'Invalidated' | 'Unverified'
export interface BoardQuery { last_n?: number; origins?: ('Engine' | 'Node')[]; status?: BoardValidity; keyword?: string; entry_id?: string }
export interface BoardTotals { completed: number; failed: number; passed_checks: number; failed_checks: number; duration_ms: number; reported_tokens: number; tokens_complete: boolean }
export interface BoardEntry { id: string; sequence: number; run_id: string; origin: 'Engine' | 'Node'; evidence_kind: string; event: string; validity: BoardValidity; node_id: string | null; scope: string | null; frame: string[]; attempt: number | null; version: VersionRef | null; evidence_refs: string[]; invalidates: string[]; note: string; digest: string | null }
export interface Blackboard { reviews?: Array<{ review_id: string; status: string; summary: string; notes: string[]; verdict: string | null; actual_action_refs: string[] }>; run_id: string; available: boolean; historical: BoardTotals; current: BoardTotals; total_entries: number; matched_entries: number; truncated: boolean; entries: BoardEntry[] }
