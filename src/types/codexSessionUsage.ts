export interface CodexSessionUsageTotals {
  requestCount: number;
  inputTokens: number;
  cachedInputTokens: number;
  outputTokens: number;
  reasoningOutputTokens: number;
  totalTokens: number;
  estimatedCostUsd: number;
  pricedRequestCount: number;
}

export interface CodexSessionUsageTrend {
  bucket: string;
  requestCount: number;
  inputTokens: number;
  cachedInputTokens: number;
  outputTokens: number;
  estimatedCostUsd: number;
}

export interface CodexSessionUsageModelStats {
  modelId: string;
  requestCount: number;
  inputTokens: number;
  cachedInputTokens: number;
  outputTokens: number;
  totalTokens: number;
  estimatedCostUsd: number;
  pricedRequestCount: number;
}

export interface CodexSessionUsageSummary {
  startAt: number;
  endAt: number;
  updatedAt: number;
  totals: CodexSessionUsageTotals;
  trends: CodexSessionUsageTrend[];
  models: CodexSessionUsageModelStats[];
  filesScanned: number;
  filesUpdated: number;
  parseErrors: number;
}

export interface CodexSessionUsageEvent {
  requestId: string;
  threadId: string;
  timestamp: number;
  modelId: string;
  providerId: string;
  originator: string;
  sourceLabel: string;
  reasoningEffort: string;
  inputTokens: number;
  cachedInputTokens: number;
  outputTokens: number;
  reasoningOutputTokens: number;
  totalTokens: number;
  estimatedCostUsd: number;
  priced: boolean;
}

export interface CodexSessionUsageEventPage {
  events: CodexSessionUsageEvent[];
  total: number;
  page: number;
  pageSize: number;
  totalPages: number;
}
