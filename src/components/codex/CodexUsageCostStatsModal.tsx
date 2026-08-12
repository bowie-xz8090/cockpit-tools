import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  BarChart3,
  Database,
  DollarSign,
  FileSearch,
  LineChart,
  RefreshCw,
  TableProperties,
  X,
} from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { CodexAccount } from '../../types/codex';
import type {
  CodexSessionUsageEventPage,
  CodexSessionUsageSummary,
} from '../../types/codexSessionUsage';
import type { ModelProviderUsageSummary } from '../../services/modelProviderUsageService';
import {
  formatModelProviderUsageInteger,
  formatModelProviderUsageMoney,
  formatModelProviderUsageTokenCount,
  resolveNewApiQuotaSnapshot,
} from '../../services/modelProviderUsageService';
import {
  queryCodexSessionUsageEvents,
  queryCodexSessionUsageSummary,
} from '../../services/codexSessionUsageService';
import { CodexStatsRangePicker } from '../CodexStatsRangePicker';
import {
  buildCodexStatsTimeRange,
  type CodexStatsRangeKey,
  type CodexStatsTimeRange,
} from '../../utils/codexStatsRange';
import { useEscClose } from '../../hooks/useEscClose';
import './CodexUsageCostStatsModal.css';

type StatsTab = 'overview' | 'trends' | 'logs' | 'models';
type JsonRecord = Record<string, unknown>;

interface CodexUsageCostStatsModalProps {
  account: CodexAccount;
  remoteSummary?: ModelProviderUsageSummary;
  remoteLoading?: boolean;
  remoteError?: string;
  remoteUnavailable?: boolean;
  remoteUpdatedAt?: number;
  onRefreshRemote?: () => Promise<void> | void;
  onClose: () => void;
}

interface RemoteMetric {
  key: string;
  label: string;
  value: string;
  detail?: string;
}

function toRecord(value: unknown): JsonRecord | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as JsonRecord)
    : null;
}

function readNumber(record: JsonRecord | null, ...keys: string[]): number | null {
  for (const key of keys) {
    const value = record?.[key];
    if (typeof value === 'number' && Number.isFinite(value)) return value;
    if (typeof value === 'string' && value.trim()) {
      const parsed = Number(value);
      if (Number.isFinite(parsed)) return parsed;
    }
  }
  return null;
}

function readString(record: JsonRecord | null, ...keys: string[]): string {
  for (const key of keys) {
    const value = record?.[key];
    if (typeof value === 'string' && value.trim()) return value.trim();
  }
  return '';
}

function getRemoteUsageRecords(account: CodexAccount) {
  const raw = toRecord(account.quota?.raw_data);
  const profile = toRecord(raw?.profile);
  const usage = toRecord(raw?.usage) ?? toRecord(profile?.usage);
  const stats = toRecord(usage?.stats);
  return {
    usage,
    requests: toRecord(stats?.requests),
    tokens: toRecord(stats?.tokens),
    total: toRecord(stats?.total),
  };
}

function formatUsd(value: number | null | undefined): string {
  if (typeof value !== 'number' || !Number.isFinite(value)) return '-';
  if (value === 0) return '$0.00';
  if (value < 0.01) return `$${value.toFixed(4)}`;
  return `$${value.toFixed(2)}`;
}

function formatDateTime(value: number | null | undefined): string {
  if (typeof value !== 'number' || !Number.isFinite(value) || value <= 0) return '-';
  return new Intl.DateTimeFormat(undefined, {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  }).format(new Date(value));
}

function formatBucket(bucket: string): string {
  const normalized = bucket.includes(' ') ? bucket.replace(' ', 'T') : `${bucket}T00:00:00`;
  const date = new Date(normalized);
  if (!Number.isFinite(date.getTime())) return bucket;
  return bucket.includes(' ')
    ? date.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })
    : date.toLocaleDateString(undefined, { month: '2-digit', day: '2-digit' });
}

export function CodexUsageCostStatsModal({
  account,
  remoteSummary,
  remoteLoading = false,
  remoteError,
  remoteUnavailable = false,
  remoteUpdatedAt,
  onRefreshRemote,
  onClose,
}: CodexUsageCostStatsModalProps) {
  const { t } = useTranslation();
  const [activeTab, setActiveTab] = useState<StatsTab>('overview');
  const [statsRange, setStatsRange] = useState<CodexStatsRangeKey>('daily');
  const [statsTimeRange, setStatsTimeRange] = useState<CodexStatsTimeRange>(() =>
    buildCodexStatsTimeRange('daily'),
  );
  const [sessionSummary, setSessionSummary] = useState<CodexSessionUsageSummary | null>(null);
  const [sessionLoading, setSessionLoading] = useState(true);
  const [sessionError, setSessionError] = useState('');
  const [logResult, setLogResult] = useState<CodexSessionUsageEventPage | null>(null);
  const [logLoading, setLogLoading] = useState(false);
  const [logError, setLogError] = useState('');
  const [logPage, setLogPage] = useState(1);
  const [logModelQuery, setLogModelQuery] = useState('');

  useEscClose(true, onClose);

  const loadSessionSummary = useCallback(async () => {
    setSessionLoading(true);
    setSessionError('');
    try {
      setSessionSummary(
        await queryCodexSessionUsageSummary(
          statsTimeRange.startAt,
          statsTimeRange.endAt,
        ),
      );
    } catch (error) {
      setSessionError(String(error).replace(/^Error:\s*/, ''));
    } finally {
      setSessionLoading(false);
    }
  }, [statsTimeRange.endAt, statsTimeRange.startAt]);

  useEffect(() => {
    void loadSessionSummary();
  }, [loadSessionSummary]);

  useEffect(() => {
    setLogPage(1);
  }, [logModelQuery, statsTimeRange.endAt, statsTimeRange.startAt]);

  useEffect(() => {
    if (activeTab !== 'logs') return;
    let disposed = false;
    setLogLoading(true);
    setLogError('');
    void queryCodexSessionUsageEvents({
      page: logPage,
      pageSize: 20,
      startAt: statsTimeRange.startAt,
      endAt: statsTimeRange.endAt,
      modelQuery: logModelQuery,
    })
      .then((result) => {
        if (!disposed) setLogResult(result);
      })
      .catch((error) => {
        if (!disposed) setLogError(String(error).replace(/^Error:\s*/, ''));
      })
      .finally(() => {
        if (!disposed) setLogLoading(false);
      });
    return () => {
      disposed = true;
    };
  }, [
    activeTab,
    logModelQuery,
    logPage,
    statsTimeRange.endAt,
    statsTimeRange.startAt,
    sessionSummary?.updatedAt,
  ]);

  const remoteRecords = useMemo(() => getRemoteUsageRecords(account), [account]);
  const newApiQuota = resolveNewApiQuotaSnapshot(remoteSummary);
  const remoteMetrics = useMemo<RemoteMetric[]>(() => {
    if (remoteSummary) {
      const balance = remoteSummary.remaining ?? remoteSummary.balance;
      const quotaRemaining =
        remoteSummary.quotaUnlimited === true
          ? t('codex.newApi.quota.unlimited', '不限量')
          : newApiQuota.available != null
            ? formatModelProviderUsageMoney(newApiQuota.available, remoteSummary.unit)
            : formatModelProviderUsageMoney(balance, remoteSummary.unit);
      return [
        {
          key: 'balance',
          label: t('codex.modelProviders.usage.accountBalance', '账户余额'),
          value: quotaRemaining,
          detail:
            newApiQuota.granted != null
              ? `${t('codex.modelProviders.usage.fields.totalGranted', '总额度')} ${formatModelProviderUsageMoney(newApiQuota.granted, remoteSummary.unit)}`
              : undefined,
        },
        {
          key: 'todayRequests',
          label: t('codex.modelProviders.usage.fields.todayRequests', '今日请求'),
          value:
            remoteSummary.todayRequests == null
              ? '-'
              : formatModelProviderUsageInteger(remoteSummary.todayRequests),
        },
        {
          key: 'todayTokens',
          label: t('codex.modelProviders.usage.fields.todayTokens', '今日 Token'),
          value:
            remoteSummary.todayTotalTokens == null
              ? '-'
              : formatModelProviderUsageTokenCount(remoteSummary.todayTotalTokens),
        },
        {
          key: 'todayCost',
          label: t('codex.modelProviders.usage.fields.todayCost', '今日消耗'),
          value: formatModelProviderUsageMoney(remoteSummary.todayCost, remoteSummary.unit),
        },
        {
          key: 'totalRequests',
          label: t('codex.modelProviders.usage.fields.totalRequests', '累计请求'),
          value:
            remoteSummary.totalRequests == null
              ? '-'
              : formatModelProviderUsageInteger(remoteSummary.totalRequests),
        },
        {
          key: 'totalTokens',
          label: t('codex.modelProviders.usage.fields.totalTokens', '累计 Token'),
          value:
            remoteSummary.totalTotalTokens == null
              ? '-'
              : formatModelProviderUsageTokenCount(remoteSummary.totalTotalTokens),
        },
        {
          key: 'totalCost',
          label: t('codex.modelProviders.usage.fields.totalCost', '累计消耗'),
          value: formatModelProviderUsageMoney(remoteSummary.totalCost, remoteSummary.unit),
        },
      ];
    }

    const { usage, requests, tokens, total } = remoteRecords;
    if (!usage && !requests && !tokens && !total) return [];
    const remaining = readNumber(usage, 'total_available', 'remaining', 'balance');
    const granted = readNumber(usage, 'total_granted', 'quota_limit');
    const totalCostDisplay =
      readString(total, 'quota_display', 'cost_display', 'total_cost_display') ||
      formatUsd(readNumber(total, 'cost', 'cost_usd', 'total_cost'));
    return [
      {
        key: 'balance',
        label: t('codex.modelProviders.usage.accountBalance', '账户余额'),
        value:
          usage?.unlimited_quota === true
            ? t('codex.newApi.quota.unlimited', '不限量')
            : remaining == null
              ? '-'
              : formatUsd(remaining),
        detail:
          granted == null
            ? undefined
            : `${t('codex.modelProviders.usage.fields.totalGranted', '总额度')} ${formatUsd(granted)}`,
      },
      {
        key: 'todayRequests',
        label: t('codex.modelProviders.usage.fields.todayRequests', '今日请求'),
        value: formatModelProviderUsageInteger(readNumber(requests, 'today') ?? 0),
      },
      {
        key: 'totalRequests',
        label: t('codex.modelProviders.usage.fields.totalRequests', '累计请求'),
        value: formatModelProviderUsageInteger(readNumber(requests, 'total') ?? 0),
      },
      {
        key: 'totalTokens',
        label: t('codex.modelProviders.usage.fields.totalTokens', '累计 Token'),
        value: formatModelProviderUsageTokenCount(readNumber(tokens, 'total') ?? 0),
      },
      {
        key: 'totalCost',
        label: t('codex.modelProviders.usage.fields.totalCost', '累计消耗'),
        value: totalCostDisplay,
      },
    ];
  }, [newApiQuota.available, newApiQuota.granted, remoteRecords, remoteSummary, t]);

  const totals = sessionSummary?.totals;
  const cacheRate = totals && totals.inputTokens > 0
    ? (totals.cachedInputTokens / totals.inputTokens) * 100
    : 0;
  const maxTrendTokens = Math.max(
    1,
    ...(sessionSummary?.trends ?? []).map((item) => item.inputTokens + item.outputTokens),
  );

  const handleRefresh = async () => {
    await Promise.allSettled([
      Promise.resolve(onRefreshRemote?.()),
      loadSessionSummary(),
    ]);
  };

  const handlePresetChange = (
    key: Exclude<CodexStatsRangeKey, 'custom'>,
    range: CodexStatsTimeRange,
  ) => {
    setStatsRange(key);
    setStatsTimeRange(range);
  };

  const handleCustomApply = (range: CodexStatsTimeRange) => {
    setStatsRange('custom');
    setStatsTimeRange(range);
  };

  const displayName = account.account_name || account.email || account.api_provider_name || account.id;
  const tabs: Array<{ key: StatsTab; icon: typeof BarChart3; label: string }> = [
    { key: 'overview', icon: BarChart3, label: t('dashboard.costStats.overview', '用量与成本') },
    { key: 'trends', icon: LineChart, label: t('dashboard.costStats.trends', '使用趋势') },
    { key: 'logs', icon: FileSearch, label: t('dashboard.costStats.requestLogs', '请求日志') },
    { key: 'models', icon: TableProperties, label: t('dashboard.costStats.modelStats', '模型统计') },
  ];

  return (
    <div className="modal-overlay codex-usage-stats-overlay" onMouseDown={onClose}>
      <div
        className="codex-usage-stats-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="codex-usage-stats-title"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <header className="codex-usage-stats-header">
          <div>
            <h2 id="codex-usage-stats-title">
              {t('dashboard.costStats.title', '用量、请求日志和成本统计')}
            </h2>
            <p>{displayName}</p>
          </div>
          <div className="codex-usage-stats-header-actions">
            <button
              type="button"
              className="codex-usage-stats-icon-btn"
              onClick={() => void handleRefresh()}
              disabled={remoteLoading || sessionLoading}
              title={t('common.refresh', '刷新')}
              aria-label={t('common.refresh', '刷新')}
            >
              <RefreshCw size={16} className={remoteLoading || sessionLoading ? 'loading-spinner' : ''} />
            </button>
            <button
              type="button"
              className="codex-usage-stats-icon-btn"
              onClick={onClose}
              title={t('common.close', '关闭')}
              aria-label={t('common.close', '关闭')}
            >
              <X size={18} />
            </button>
          </div>
        </header>

        <div className="codex-usage-stats-toolbar">
          <div className="codex-usage-stats-tabs" role="tablist">
            {tabs.map(({ key, icon: Icon, label }) => (
              <button
                key={key}
                type="button"
                role="tab"
                className={activeTab === key ? 'active' : ''}
                aria-selected={activeTab === key}
                onClick={() => setActiveTab(key)}
              >
                <Icon size={15} />
                {label}
              </button>
            ))}
          </div>
          <CodexStatsRangePicker
            value={statsRange}
            range={statsTimeRange}
            onPresetChange={handlePresetChange}
            onCustomApply={handleCustomApply}
            disabled={sessionLoading || logLoading}
            error={sessionError}
            compact
          />
        </div>

        <div className="codex-usage-stats-body">
          {activeTab === 'overview' && (
            <>
              <section className="codex-usage-stats-section">
                <div className="codex-usage-stats-section-heading">
                  <div>
                    <h3>{t('dashboard.costStats.sessionSummary', 'Codex 本地会话统计')}</h3>
                    <p>{t('dashboard.costStats.sessionSummaryHint', '读取 Codex 会话 JSONL，包含直连供应商且未经过 API Service 的 Codex 请求。')}</p>
                  </div>
                  {sessionSummary?.updatedAt ? <span>{formatDateTime(sessionSummary.updatedAt)}</span> : null}
                </div>
                <div className="codex-usage-stats-scope-notice">
                  <Database size={15} />
                  <span>{t('dashboard.costStats.sessionScopeHint', '会话文件不包含完整 API Key；以下会话统计覆盖本机 Codex 客户端，不能保证全部属于当前账号卡片。')}</span>
                </div>
                {sessionError && <div className="codex-usage-stats-notice error">{sessionError}</div>}
                {totals && totals.requestCount > 0 ? (
                  <div className="codex-usage-stats-hero-grid">
                    <article>
                      <span>{t('dashboard.costStats.realTokens', '实际消耗 Token')}</span>
                      <strong>{formatModelProviderUsageTokenCount(totals.totalTokens)}</strong>
                      <small>{t('dashboard.costStats.tokenBreakdown', '输入 {{input}} / 输出 {{output}}', {
                        input: formatModelProviderUsageTokenCount(totals.inputTokens),
                        output: formatModelProviderUsageTokenCount(totals.outputTokens),
                      })}</small>
                    </article>
                    <article>
                      <span>{t('dashboard.costStats.cacheHitRate', '缓存命中率')}</span>
                      <strong>{cacheRate.toFixed(1)}%</strong>
                      <small>{t('dashboard.costStats.cachedTokens', '缓存读取 {{tokens}}', {
                        tokens: formatModelProviderUsageTokenCount(totals.cachedInputTokens),
                      })}</small>
                    </article>
                    <article>
                      <span>{t('dashboard.costStats.requests', '请求数')}</span>
                      <strong>{formatModelProviderUsageInteger(totals.requestCount)}</strong>
                      <small>{t('dashboard.costStats.sessionImported', '来自 Codex 会话日志')}</small>
                    </article>
                    <article className="accent">
                      <span>{t('codex.localAccess.stats.estimatedCost', '估算价值')}</span>
                      <strong>{formatUsd(totals.estimatedCostUsd)}</strong>
                      <small>{t('dashboard.costStats.pricedCount', '{{priced}} / {{total}} 条已匹配价格', {
                        priced: totals.pricedRequestCount,
                        total: totals.requestCount,
                      })}</small>
                    </article>
                  </div>
                ) : (
                  <div className="codex-usage-stats-empty">
                    {sessionLoading
                      ? t('dashboard.costStats.scanningSessions', '正在扫描 Codex 会话日志...')
                      : t('dashboard.costStats.sessionEmpty', '所选时间范围内没有可识别的 Codex 会话用量。')}
                  </div>
                )}
                {sessionSummary && (sessionSummary.filesUpdated > 0 || sessionSummary.parseErrors > 0) && (
                  <p className="codex-usage-stats-scan-meta">
                    {t('dashboard.costStats.scanMeta', '扫描 {{files}} 个文件，更新 {{updated}} 个，解析异常 {{errors}} 个', {
                      files: sessionSummary.filesScanned,
                      updated: sessionSummary.filesUpdated,
                      errors: sessionSummary.parseErrors,
                    })}
                  </p>
                )}
              </section>

              <section className="codex-usage-stats-section">
                <div className="codex-usage-stats-section-heading">
                  <div>
                    <h3>{t('dashboard.costStats.remoteSummary', '供应商 usage 汇总')}</h3>
                    <p>{t('dashboard.costStats.remoteSummaryHint', '数据由当前 API 供应商返回，统计口径以供应商为准。')}</p>
                  </div>
                  {remoteUpdatedAt && <span>{formatDateTime(remoteUpdatedAt)}</span>}
                </div>
                {remoteError && <div className="codex-usage-stats-notice error">{remoteError}</div>}
                {remoteUnavailable && (
                  <div className="codex-usage-stats-notice">
                    {t('dashboard.costStats.remoteUnavailable', '当前供应商未提供可识别的 usage 接口。')}
                  </div>
                )}
                {remoteMetrics.length > 0 ? (
                  <div className="codex-usage-stats-metric-grid">
                    {remoteMetrics.map((metric) => (
                      <article key={metric.key} className="codex-usage-stats-metric">
                        <span>{metric.label}</span>
                        <strong>{metric.value}</strong>
                        {metric.detail && <small>{metric.detail}</small>}
                      </article>
                    ))}
                  </div>
                ) : !remoteLoading && !remoteError && !remoteUnavailable ? (
                  <div className="codex-usage-stats-empty">
                    {t('dashboard.costStats.remoteEmpty', '暂无供应商 usage 汇总，可点击刷新后重试。')}
                  </div>
                ) : null}
              </section>
            </>
          )}

          {activeTab === 'trends' && (
            <section className="codex-usage-stats-section codex-usage-stats-trends">
              <div className="codex-usage-stats-section-heading">
                <div>
                  <h3>{t('dashboard.costStats.trends', '使用趋势')}</h3>
                  <p>{t('dashboard.costStats.trendsHint', '蓝色为输入 Token，绿色为输出 Token；成本按当前价格快照估算。')}</p>
                </div>
              </div>
              {(sessionSummary?.trends.length ?? 0) > 0 ? (
                <div className="codex-usage-stats-chart" role="img" aria-label={t('dashboard.costStats.trends', '使用趋势')}>
                  <div className="codex-usage-stats-chart-legend">
                    <span className="input">{t('dashboard.costStats.inputTokens', '输入 Token')}</span>
                    <span className="output">{t('dashboard.costStats.outputTokens', '输出 Token')}</span>
                  </div>
                  <div className="codex-usage-stats-chart-plot">
                    {(sessionSummary?.trends ?? []).map((point) => (
                      <div className="codex-usage-stats-chart-column" key={point.bucket} title={`${formatBucket(point.bucket)} · ${formatModelProviderUsageTokenCount(point.inputTokens + point.outputTokens)} · ${formatUsd(point.estimatedCostUsd)}`}>
                        <div className="codex-usage-stats-chart-bars">
                          <span className="input" style={{ height: `${Math.max(2, (point.inputTokens / maxTrendTokens) * 100)}%` }} />
                          <span className="output" style={{ height: `${Math.max(2, (point.outputTokens / maxTrendTokens) * 100)}%` }} />
                        </div>
                        <small>{formatBucket(point.bucket)}</small>
                      </div>
                    ))}
                  </div>
                </div>
              ) : (
                <div className="codex-usage-stats-empty">{t('dashboard.costStats.sessionEmpty', '所选时间范围内没有可识别的 Codex 会话用量。')}</div>
              )}
            </section>
          )}

          {activeTab === 'logs' && (
            <section className="codex-usage-stats-section logs">
              <div className="codex-usage-stats-log-filters">
                <input
                  type="search"
                  value={logModelQuery}
                  onChange={(event) => setLogModelQuery(event.target.value)}
                  placeholder={t('dashboard.costStats.searchModel', '筛选模型...')}
                />
                <span>{t('dashboard.costStats.logCount', '共 {{count}} 条', { count: logResult?.total ?? 0 })}</span>
              </div>
              <div className="codex-usage-stats-scope-notice compact">
                <Database size={14} />
                <span>{t('dashboard.costStats.sessionLogHint', '这些记录由 Codex 会话 token_count 导入，不包含真实 HTTP 状态、错误或延迟。')}</span>
              </div>
              {logError && <div className="codex-usage-stats-notice error">{logError}</div>}
              <div className="codex-usage-stats-table-wrap">
                <table className="codex-usage-stats-table session-logs">
                  <thead>
                    <tr>
                      <th>{t('dashboard.costStats.time', '时间')}</th>
                      <th>{t('dashboard.costStats.client', '客户端')}</th>
                      <th>{t('dashboard.costStats.internalSource', '内部来源')}</th>
                      <th>{t('dashboard.costStats.model', '模型')}</th>
                      <th>{t('dashboard.costStats.reasoningEffort', '推理强度')}</th>
                      <th>{t('dashboard.costStats.inputTokens', '输入 Token')}</th>
                      <th>{t('dashboard.costStats.outputTokens', '输出 Token')}</th>
                      <th>{t('dashboard.costStats.cost', '成本')}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {(logResult?.events ?? []).map((event) => (
                      <tr key={event.requestId}>
                        <td>{formatDateTime(event.timestamp)}</td>
                        <td title={event.originator || undefined}>{event.originator || t('dashboard.costStats.unknown', '未知')}</td>
                        <td><span className="codex-usage-stats-source-badge">{event.sourceLabel || 'codex_session'}</span></td>
                        <td title={event.modelId}>{event.modelId || '-'}</td>
                        <td>{event.reasoningEffort || t('dashboard.costStats.unknown', '未知')}</td>
                        <td>
                          {formatModelProviderUsageTokenCount(event.inputTokens)}
                          {event.cachedInputTokens > 0 && <small>{t('dashboard.costStats.cacheShort', '缓存 {{tokens}}', { tokens: formatModelProviderUsageTokenCount(event.cachedInputTokens) })}</small>}
                        </td>
                        <td>{formatModelProviderUsageTokenCount(event.outputTokens)}</td>
                        <td>{event.priced ? formatUsd(event.estimatedCostUsd) : t('dashboard.costStats.unpriced', '未定价')}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
                {!logLoading && (logResult?.events.length ?? 0) === 0 && (
                  <div className="codex-usage-stats-empty table-empty">
                    {t('dashboard.costStats.sessionLogsEmpty', '没有匹配的 Codex 会话日志。')}
                  </div>
                )}
                {logLoading && (
                  <div className="codex-usage-stats-loading">
                    <RefreshCw size={18} className="loading-spinner" />
                    {t('common.loading', '加载中...')}
                  </div>
                )}
              </div>
              <div className="codex-usage-stats-pagination">
                <button type="button" className="btn btn-secondary btn-sm" disabled={logPage <= 1 || logLoading} onClick={() => setLogPage((value) => Math.max(1, value - 1))}>
                  {t('common.previous', '上一页')}
                </button>
                <span>{logResult?.page ?? logPage} / {logResult?.totalPages ?? 1}</span>
                <button type="button" className="btn btn-secondary btn-sm" disabled={logPage >= (logResult?.totalPages ?? 1) || logLoading} onClick={() => setLogPage((value) => value + 1)}>
                  {t('common.next', '下一页')}
                </button>
              </div>
            </section>
          )}

          {activeTab === 'models' && (
            <section className="codex-usage-stats-section logs">
              <div className="codex-usage-stats-section-heading">
                <div>
                  <h3>{t('dashboard.costStats.modelStats', '模型统计')}</h3>
                  <p>{t('dashboard.costStats.modelStatsHint', '按会话中记录的计费模型聚合 Token 和估算成本。')}</p>
                </div>
              </div>
              <div className="codex-usage-stats-table-wrap model-stats">
                <table className="codex-usage-stats-table model-stats">
                  <thead>
                    <tr>
                      <th>{t('dashboard.costStats.model', '模型')}</th>
                      <th>{t('dashboard.costStats.requests', '请求数')}</th>
                      <th>{t('dashboard.costStats.inputTokens', '输入 Token')}</th>
                      <th>{t('dashboard.costStats.outputTokens', '输出 Token')}</th>
                      <th>{t('dashboard.costStats.tokens', 'Token')}</th>
                      <th>{t('dashboard.costStats.cost', '成本')}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {(sessionSummary?.models ?? []).map((model) => (
                      <tr key={model.modelId}>
                        <td title={model.modelId}>{model.modelId}</td>
                        <td>{formatModelProviderUsageInteger(model.requestCount)}</td>
                        <td>{formatModelProviderUsageTokenCount(model.inputTokens)}</td>
                        <td>{formatModelProviderUsageTokenCount(model.outputTokens)}</td>
                        <td>{formatModelProviderUsageTokenCount(model.totalTokens)}</td>
                        <td>{model.pricedRequestCount > 0 ? formatUsd(model.estimatedCostUsd) : t('dashboard.costStats.unpriced', '未定价')}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
                {!sessionLoading && (sessionSummary?.models.length ?? 0) === 0 && (
                  <div className="codex-usage-stats-empty table-empty">{t('dashboard.costStats.sessionEmpty', '所选时间范围内没有可识别的 Codex 会话用量。')}</div>
                )}
              </div>
            </section>
          )}
        </div>

        <footer className="codex-usage-stats-footer">
          <span>
            <DollarSign size={14} />
            {t('dashboard.costStats.footerHint', '会话成本为模型价格估算值，供应商账单为准。')}
          </span>
          <div>
            <button type="button" className="btn btn-primary" onClick={onClose}>
              {t('common.close', '关闭')}
            </button>
          </div>
        </footer>
      </div>
    </div>
  );
}

export default CodexUsageCostStatsModal;
