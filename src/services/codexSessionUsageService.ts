import { invoke } from '@tauri-apps/api/core';
import type {
  CodexSessionUsageEventPage,
  CodexSessionUsageSummary,
} from '../types/codexSessionUsage';

export async function queryCodexSessionUsageSummary(
  startAt: number,
  endAt: number,
): Promise<CodexSessionUsageSummary> {
  return await invoke('codex_query_session_usage_summary', { startAt, endAt });
}

export async function queryCodexSessionUsageEvents(input: {
  startAt: number;
  endAt: number;
  page: number;
  pageSize: number;
  modelQuery?: string;
}): Promise<CodexSessionUsageEventPage> {
  return await invoke('codex_query_session_usage_events', {
    startAt: input.startAt,
    endAt: input.endAt,
    page: input.page,
    pageSize: input.pageSize,
    modelQuery: input.modelQuery?.trim() || null,
  });
}
