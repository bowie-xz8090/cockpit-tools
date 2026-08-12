use crate::modules::{account, codex_account, codex_local_access};
use chrono::{DateTime, Local};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::UNIX_EPOCH;

const SESSION_USAGE_DB_FILE: &str = "codex_session_usage.sqlite";
const SESSION_USAGE_SCHEMA_VERSION: i64 = 3;
const MAX_SESSION_DEPTH: usize = 4;

static SESSION_USAGE_SYNC_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[derive(Debug, Clone, Default)]
struct TokenCounters {
    input: u64,
    cached_input: u64,
    output: u64,
    reasoning_output: u64,
    total: u64,
}

#[derive(Debug, Clone, Default)]
struct SessionCursor {
    byte_offset: u64,
    file_size: u64,
    modified_ms: i64,
    current_model: String,
    thread_id: String,
    provider_id: String,
    originator: String,
    source_label: String,
    reasoning_effort: String,
    total: TokenCounters,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CodexSessionUsageTotals {
    pub request_count: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
    pub estimated_cost_usd: f64,
    pub priced_request_count: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexSessionUsageTrend {
    pub bucket: String,
    pub request_count: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub estimated_cost_usd: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexSessionUsageModelStats {
    pub model_id: String,
    pub request_count: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub estimated_cost_usd: f64,
    pub priced_request_count: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexSessionUsageSummary {
    pub start_at: i64,
    pub end_at: i64,
    pub updated_at: i64,
    pub totals: CodexSessionUsageTotals,
    pub trends: Vec<CodexSessionUsageTrend>,
    pub models: Vec<CodexSessionUsageModelStats>,
    pub files_scanned: u64,
    pub files_updated: u64,
    pub parse_errors: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexSessionUsageEvent {
    pub request_id: String,
    pub thread_id: String,
    pub timestamp: i64,
    pub model_id: String,
    pub provider_id: String,
    pub originator: String,
    pub source_label: String,
    pub reasoning_effort: String,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
    pub estimated_cost_usd: f64,
    pub priced: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexSessionUsageEventPage {
    pub events: Vec<CodexSessionUsageEvent>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
    pub total_pages: u64,
}

#[derive(Debug, Default)]
struct SyncResult {
    files_scanned: u64,
    files_updated: u64,
    parse_errors: u64,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn session_usage_db_path() -> Result<PathBuf, String> {
    Ok(account::get_data_dir()?.join(SESSION_USAGE_DB_FILE))
}

fn ensure_column(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), String> {
    let mut statement = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|error| format!("读取 Codex 会话统计表结构失败: {error}"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| format!("查询 Codex 会话统计表结构失败: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("解析 Codex 会话统计表结构失败: {error}"))?;
    if !columns.iter().any(|existing| existing == column) {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
            [],
        )
        .map_err(|error| format!("升级 Codex 会话统计字段失败: {error}"))?;
    }
    Ok(())
}

fn open_session_usage_db() -> Result<Connection, String> {
    let path = session_usage_db_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("创建 Codex 会话统计目录失败: {error}"))?;
    }
    let conn =
        Connection::open(path).map_err(|error| format!("打开 Codex 会话统计缓存失败: {error}"))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| format!("配置 Codex 会话统计缓存失败: {error}"))?;
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        CREATE TABLE IF NOT EXISTS session_usage_events (
            event_key TEXT PRIMARY KEY,
            request_id TEXT NOT NULL,
            thread_id TEXT NOT NULL DEFAULT '',
            source_path TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            model_id TEXT NOT NULL DEFAULT '',
            provider_id TEXT NOT NULL DEFAULT '',
            originator TEXT NOT NULL DEFAULT '',
            source_label TEXT NOT NULL DEFAULT '',
            reasoning_effort TEXT NOT NULL DEFAULT '',
            input_tokens INTEGER NOT NULL DEFAULT 0,
            cached_input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            reasoning_output_tokens INTEGER NOT NULL DEFAULT 0,
            total_tokens INTEGER NOT NULL DEFAULT 0,
            estimated_cost_usd REAL NOT NULL DEFAULT 0,
            priced INTEGER NOT NULL DEFAULT 0,
            pricing_version INTEGER NOT NULL DEFAULT 0,
            input_usd_per_million REAL NOT NULL DEFAULT 0,
            cached_input_usd_per_million REAL NOT NULL DEFAULT 0,
            output_usd_per_million REAL NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_session_usage_timestamp
            ON session_usage_events(timestamp DESC);
        CREATE INDEX IF NOT EXISTS idx_session_usage_model
            ON session_usage_events(model_id, timestamp DESC);
        CREATE INDEX IF NOT EXISTS idx_session_usage_source_path
            ON session_usage_events(source_path);
        CREATE TABLE IF NOT EXISTS session_usage_cursors (
            source_path TEXT PRIMARY KEY,
            byte_offset INTEGER NOT NULL DEFAULT 0,
            file_size INTEGER NOT NULL DEFAULT 0,
            modified_ms INTEGER NOT NULL DEFAULT 0,
            current_model TEXT NOT NULL DEFAULT '',
            thread_id TEXT NOT NULL DEFAULT '',
            provider_id TEXT NOT NULL DEFAULT '',
            originator TEXT NOT NULL DEFAULT '',
            source_label TEXT NOT NULL DEFAULT '',
            reasoning_effort TEXT NOT NULL DEFAULT '',
            total_input INTEGER NOT NULL DEFAULT 0,
            total_cached_input INTEGER NOT NULL DEFAULT 0,
            total_output INTEGER NOT NULL DEFAULT 0,
            total_reasoning_output INTEGER NOT NULL DEFAULT 0,
            total_tokens INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS session_usage_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL DEFAULT ''
        );
        "#,
    )
    .map_err(|error| format!("初始化 Codex 会话统计缓存失败: {error}"))?;
    ensure_column(
        &conn,
        "session_usage_events",
        "originator",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    ensure_column(
        &conn,
        "session_usage_events",
        "reasoning_effort",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    ensure_column(
        &conn,
        "session_usage_cursors",
        "originator",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    ensure_column(
        &conn,
        "session_usage_cursors",
        "reasoning_effort",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    let stored_version = conn
        .query_row(
            "SELECT value FROM session_usage_meta WHERE key = 'schema_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| format!("读取 Codex 会话统计版本失败: {error}"))?
        .and_then(|value| value.parse::<i64>().ok());
    if stored_version != Some(SESSION_USAGE_SCHEMA_VERSION) {
        conn.execute_batch("DELETE FROM session_usage_events; DELETE FROM session_usage_cursors;")
            .map_err(|error| format!("迁移 Codex 会话统计缓存失败: {error}"))?;
    }
    conn.execute(
        "INSERT OR REPLACE INTO session_usage_meta (key, value) VALUES ('schema_version', ?1)",
        [SESSION_USAGE_SCHEMA_VERSION.to_string()],
    )
    .map_err(|error| format!("写入 Codex 会话统计版本失败: {error}"))?;
    Ok(conn)
}

fn metadata_modified_ms(metadata: &fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default()
}

fn collect_jsonl_files(dir: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    if depth > MAX_SESSION_DEPTH || !dir.is_dir() {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl_files(&path, depth + 1, files);
        } else if path.extension().and_then(|value| value.to_str()) == Some("jsonl") {
            files.push(path);
        }
    }
}

fn session_started_ms(path: &Path) -> Option<i64> {
    let name = path.file_name()?.to_str()?;
    let date = name.strip_prefix("rollout-")?.get(..10)?;
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()?
        .and_hms_opt(0, 0, 0)?
        .and_local_timezone(Local)
        .earliest()
        .map(|value| value.timestamp_millis())
}

fn relevant_session_files(start_at: i64, end_at: i64) -> Vec<PathBuf> {
    let codex_home = codex_account::get_codex_home();
    let mut files = Vec::new();
    collect_jsonl_files(&codex_home.join("sessions"), 0, &mut files);
    collect_jsonl_files(&codex_home.join("archived_sessions"), 0, &mut files);
    files.retain(|path| {
        let Ok(metadata) = fs::metadata(path) else {
            return false;
        };
        let started = session_started_ms(path).unwrap_or_default();
        started <= end_at && metadata_modified_ms(&metadata) >= start_at
    });
    files.sort();
    files
}

fn load_cursor(conn: &Connection, source_path: &str) -> Result<SessionCursor, String> {
    conn.query_row(
        r#"
        SELECT byte_offset, file_size, modified_ms, current_model, thread_id,
               provider_id, originator, source_label, reasoning_effort,
               total_input, total_cached_input, total_output,
               total_reasoning_output, total_tokens
        FROM session_usage_cursors WHERE source_path = ?1
        "#,
        [source_path],
        |row| {
            Ok(SessionCursor {
                byte_offset: row.get(0)?,
                file_size: row.get(1)?,
                modified_ms: row.get(2)?,
                current_model: row.get(3)?,
                thread_id: row.get(4)?,
                provider_id: row.get(5)?,
                originator: row.get(6)?,
                source_label: row.get(7)?,
                reasoning_effort: row.get(8)?,
                total: TokenCounters {
                    input: row.get(9)?,
                    cached_input: row.get(10)?,
                    output: row.get(11)?,
                    reasoning_output: row.get(12)?,
                    total: row.get(13)?,
                },
            })
        },
    )
    .optional()
    .map(|value| value.unwrap_or_default())
    .map_err(|error| format!("读取 Codex 会话同步游标失败: {error}"))
}

fn save_cursor(conn: &Connection, source_path: &str, cursor: &SessionCursor) -> Result<(), String> {
    conn.execute(
        r#"
        INSERT INTO session_usage_cursors (
            source_path, byte_offset, file_size, modified_ms, current_model,
            thread_id, provider_id, originator, source_label, reasoning_effort,
            total_input, total_cached_input, total_output,
            total_reasoning_output, total_tokens
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
        ON CONFLICT(source_path) DO UPDATE SET
            byte_offset = excluded.byte_offset,
            file_size = excluded.file_size,
            modified_ms = excluded.modified_ms,
            current_model = excluded.current_model,
            thread_id = excluded.thread_id,
            provider_id = excluded.provider_id,
            originator = excluded.originator,
            source_label = excluded.source_label,
            reasoning_effort = excluded.reasoning_effort,
            total_input = excluded.total_input,
            total_cached_input = excluded.total_cached_input,
            total_output = excluded.total_output,
            total_reasoning_output = excluded.total_reasoning_output,
            total_tokens = excluded.total_tokens
        "#,
        params![
            source_path,
            cursor.byte_offset,
            cursor.file_size,
            cursor.modified_ms,
            cursor.current_model,
            cursor.thread_id,
            cursor.provider_id,
            cursor.originator,
            cursor.source_label,
            cursor.reasoning_effort,
            cursor.total.input,
            cursor.total.cached_input,
            cursor.total.output,
            cursor.total.reasoning_output,
            cursor.total.total,
        ],
    )
    .map_err(|error| format!("保存 Codex 会话同步游标失败: {error}"))?;
    Ok(())
}

fn value_u64(value: Option<&Value>, keys: &[&str]) -> u64 {
    for key in keys {
        if let Some(number) = value
            .and_then(|item| item.get(*key))
            .and_then(Value::as_u64)
        {
            return number;
        }
    }
    0
}

fn parse_counters(value: Option<&Value>) -> Option<TokenCounters> {
    let value = value?.as_object()?;
    let root = Value::Object(value.clone());
    let counters = TokenCounters {
        input: value_u64(Some(&root), &["input_tokens"]),
        cached_input: value_u64(
            Some(&root),
            &["cached_input_tokens", "cache_read_input_tokens"],
        ),
        output: value_u64(Some(&root), &["output_tokens"]),
        reasoning_output: value_u64(Some(&root), &["reasoning_output_tokens"]),
        total: value_u64(Some(&root), &["total_tokens"]),
    };
    (counters.input > 0 || counters.output > 0 || counters.total > 0).then_some(counters)
}

fn counter_delta(previous: &TokenCounters, current: &TokenCounters) -> TokenCounters {
    TokenCounters {
        input: current.input.saturating_sub(previous.input),
        cached_input: current.cached_input.saturating_sub(previous.cached_input),
        output: current.output.saturating_sub(previous.output),
        reasoning_output: current
            .reasoning_output
            .saturating_sub(previous.reasoning_output),
        total: current.total.saturating_sub(previous.total),
    }
}

fn normalize_model(raw: &str) -> String {
    let mut model = raw.trim().to_ascii_lowercase();
    if let Some((_, tail)) = model.rsplit_once('/') {
        model = tail.to_string();
    }
    model
}

fn source_label(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(label)) if !label.trim().is_empty() => label.trim().to_string(),
        Some(Value::Object(object)) if object.contains_key("subagent") => "subagent".to_string(),
        _ => "codex_session".to_string(),
    }
}

fn string_field(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
        .to_string()
}

fn reasoning_effort(payload: &Value) -> String {
    let value = payload
        .get("effort")
        .or_else(|| payload.get("reasoning_effort"))
        .or_else(|| {
            payload
                .get("collaboration_mode")
                .and_then(|mode| mode.get("settings"))
                .and_then(|settings| settings.get("reasoning_effort"))
        })
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_default();
    value.to_ascii_lowercase()
}

fn timestamp_ms(value: Option<&Value>) -> Option<i64> {
    value
        .and_then(Value::as_str)
        .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
        .map(|value| value.timestamp_millis())
}

fn thread_id_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|value| value.to_str())
        .and_then(|stem| stem.get(stem.len().saturating_sub(36)..))
        .unwrap_or_default()
        .to_string()
}

fn event_key(
    thread_id: &str,
    timestamp: i64,
    model: &str,
    total: Option<&TokenCounters>,
    delta: &TokenCounters,
) -> String {
    let signature = total.unwrap_or(delta);
    let mut digest = Sha256::new();
    digest.update(format!(
        "{thread_id}|{timestamp}|{model}|{}|{}|{}|{}|{}",
        signature.input,
        signature.cached_input,
        signature.output,
        signature.reasoning_output,
        signature.total
    ));
    format!("{:x}", digest.finalize())
}

fn sync_session_file(
    conn: &mut Connection,
    path: &Path,
    pricing_resolver: &codex_local_access::CodexSessionPricingResolver,
) -> Result<bool, String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("读取 Codex 会话文件元数据失败: {error}"))?;
    let source_path = path.to_string_lossy().to_string();
    let file_size = metadata.len();
    let modified_ms = metadata_modified_ms(&metadata);
    let mut cursor = load_cursor(conn, &source_path)?;
    if cursor.file_size == file_size && cursor.modified_ms == modified_ms {
        return Ok(false);
    }
    if file_size < cursor.byte_offset || file_size < cursor.file_size {
        conn.execute(
            "DELETE FROM session_usage_events WHERE source_path = ?1",
            [&source_path],
        )
        .map_err(|error| format!("重建截断的 Codex 会话缓存失败: {error}"))?;
        cursor = SessionCursor::default();
    }
    if cursor.thread_id.is_empty() {
        cursor.thread_id = thread_id_from_path(path);
    }

    let mut file = File::open(path).map_err(|error| format!("打开 Codex 会话文件失败: {error}"))?;
    file.seek(SeekFrom::Start(cursor.byte_offset))
        .map_err(|error| format!("定位 Codex 会话文件失败: {error}"))?;
    let mut reader = BufReader::new(file);
    let tx = conn
        .transaction()
        .map_err(|error| format!("开启 Codex 会话导入事务失败: {error}"))?;
    let mut committed_offset = cursor.byte_offset;
    let mut line = String::new();

    loop {
        line.clear();
        let line_start = reader
            .stream_position()
            .map_err(|error| format!("读取 Codex 会话偏移失败: {error}"))?;
        let bytes = reader
            .read_line(&mut line)
            .map_err(|error| format!("读取 Codex 会话文件失败: {error}"))?;
        if bytes == 0 {
            break;
        }
        if !line.ends_with('\n') {
            committed_offset = line_start;
            break;
        }
        committed_offset = reader
            .stream_position()
            .map_err(|error| format!("读取 Codex 会话偏移失败: {error}"))?;
        if !line.contains("session_meta")
            && !line.contains("turn_context")
            && !line.contains("token_count")
        {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match record.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                let payload = record.get("payload").unwrap_or(&Value::Null);
                if let Some(value) = payload.get("id").and_then(Value::as_str) {
                    cursor.thread_id = value.to_string();
                }
                if let Some(value) = payload.get("model_provider").and_then(Value::as_str) {
                    cursor.provider_id = value.to_string();
                }
                cursor.originator = string_field(payload.get("originator"));
                cursor.source_label = source_label(payload.get("source"));
            }
            Some("turn_context") => {
                let payload = record.get("payload").unwrap_or(&Value::Null);
                if let Some(value) = payload
                    .get("model")
                    .or_else(|| payload.get("info").and_then(|info| info.get("model")))
                    .and_then(Value::as_str)
                {
                    cursor.current_model = normalize_model(value);
                }
                cursor.reasoning_effort = reasoning_effort(payload);
            }
            Some("event_msg") => {
                let payload = record.get("payload").unwrap_or(&Value::Null);
                if payload.get("type").and_then(Value::as_str) != Some("token_count") {
                    continue;
                }
                let Some(info) = payload.get("info").filter(|value| !value.is_null()) else {
                    continue;
                };
                if let Some(value) = info
                    .get("model")
                    .or_else(|| info.get("model_name"))
                    .or_else(|| payload.get("model"))
                    .and_then(Value::as_str)
                {
                    cursor.current_model = normalize_model(value);
                }
                let total = parse_counters(info.get("total_token_usage"));
                let last = parse_counters(info.get("last_token_usage"));
                let duplicate_total = total.as_ref().is_some_and(|current| {
                    cursor.total.total > 0
                        && current.input == cursor.total.input
                        && current.cached_input == cursor.total.cached_input
                        && current.output == cursor.total.output
                        && current.reasoning_output == cursor.total.reasoning_output
                        && current.total == cursor.total.total
                });
                if duplicate_total {
                    continue;
                }
                let mut delta = last.unwrap_or_else(|| {
                    total
                        .as_ref()
                        .map(|current| counter_delta(&cursor.total, current))
                        .unwrap_or_default()
                });
                delta.cached_input = delta.cached_input.min(delta.input);
                if delta.total == 0 {
                    delta.total = delta.input.saturating_add(delta.output);
                }
                if let Some(current) = total.as_ref() {
                    cursor.total.input = cursor.total.input.max(current.input);
                    cursor.total.cached_input = cursor.total.cached_input.max(current.cached_input);
                    cursor.total.output = cursor.total.output.max(current.output);
                    cursor.total.reasoning_output =
                        cursor.total.reasoning_output.max(current.reasoning_output);
                    cursor.total.total = cursor.total.total.max(current.total);
                }
                if delta.input == 0 && delta.output == 0 {
                    continue;
                }
                let timestamp = timestamp_ms(record.get("timestamp")).unwrap_or(modified_ms);
                let model = if cursor.current_model.is_empty() {
                    "unknown"
                } else {
                    cursor.current_model.as_str()
                };
                let pricing = pricing_resolver.estimate(
                    model,
                    delta.input,
                    delta.output,
                    delta.cached_input,
                    delta.reasoning_output,
                );
                let key = event_key(&cursor.thread_id, timestamp, model, total.as_ref(), &delta);
                let request_id = format!(
                    "codex-session:{}:{}:{}",
                    cursor.thread_id, timestamp, line_start
                );
                tx.execute(
                    r#"
                    INSERT OR IGNORE INTO session_usage_events (
                        event_key, request_id, thread_id, source_path, timestamp,
                        model_id, provider_id, originator, source_label,
                        reasoning_effort, input_tokens, cached_input_tokens,
                        output_tokens, reasoning_output_tokens, total_tokens,
                        estimated_cost_usd, priced, pricing_version,
                        input_usd_per_million, cached_input_usd_per_million,
                        output_usd_per_million
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                              ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)
                    "#,
                    params![
                        key,
                        request_id,
                        cursor.thread_id,
                        source_path,
                        timestamp,
                        model,
                        cursor.provider_id,
                        cursor.originator,
                        cursor.source_label,
                        cursor.reasoning_effort,
                        delta.input,
                        delta.cached_input,
                        delta.output,
                        delta.reasoning_output,
                        delta.total,
                        pricing.estimated_cost_usd,
                        pricing.priced,
                        pricing.pricing_version,
                        pricing.input_usd_per_million,
                        pricing.cached_input_usd_per_million,
                        pricing.output_usd_per_million,
                    ],
                )
                .map_err(|error| format!("写入 Codex 会话用量失败: {error}"))?;
            }
            _ => {}
        }
    }

    cursor.byte_offset = committed_offset;
    cursor.file_size = file_size;
    cursor.modified_ms = modified_ms;
    save_cursor(&tx, &source_path, &cursor)?;
    tx.commit()
        .map_err(|error| format!("提交 Codex 会话用量失败: {error}"))?;
    Ok(true)
}

fn sync_session_usage(conn: &mut Connection, start_at: i64, end_at: i64) -> SyncResult {
    let files = relevant_session_files(start_at, end_at);
    let mut result = SyncResult {
        files_scanned: files.len() as u64,
        ..SyncResult::default()
    };
    let pricing_resolver = codex_local_access::CodexSessionPricingResolver::load();
    for file in files {
        match sync_session_file(conn, &file, &pricing_resolver) {
            Ok(true) => result.files_updated += 1,
            Ok(false) => {}
            Err(error) => {
                result.parse_errors += 1;
                crate::modules::logger::log_warn(&format!(
                    "[Codex Session Usage] {}: {}",
                    file.display(),
                    error
                ));
            }
        }
    }
    result
}

fn query_totals(
    conn: &Connection,
    start_at: i64,
    end_at: i64,
) -> Result<CodexSessionUsageTotals, String> {
    conn.query_row(
        r#"
        SELECT COUNT(*), COALESCE(SUM(input_tokens), 0),
               COALESCE(SUM(cached_input_tokens), 0),
               COALESCE(SUM(output_tokens), 0),
               COALESCE(SUM(reasoning_output_tokens), 0),
               COALESCE(SUM(total_tokens), 0),
               COALESCE(SUM(estimated_cost_usd), 0),
               COALESCE(SUM(priced), 0)
        FROM session_usage_events WHERE timestamp BETWEEN ?1 AND ?2
        "#,
        params![start_at, end_at],
        |row| {
            Ok(CodexSessionUsageTotals {
                request_count: row.get(0)?,
                input_tokens: row.get(1)?,
                cached_input_tokens: row.get(2)?,
                output_tokens: row.get(3)?,
                reasoning_output_tokens: row.get(4)?,
                total_tokens: row.get(5)?,
                estimated_cost_usd: row.get(6)?,
                priced_request_count: row.get(7)?,
            })
        },
    )
    .map_err(|error| format!("查询 Codex 会话用量汇总失败: {error}"))
}

fn query_models(
    conn: &Connection,
    start_at: i64,
    end_at: i64,
) -> Result<Vec<CodexSessionUsageModelStats>, String> {
    let mut statement = conn
        .prepare(
            r#"
            SELECT model_id, COUNT(*), COALESCE(SUM(input_tokens), 0),
                   COALESCE(SUM(cached_input_tokens), 0),
                   COALESCE(SUM(output_tokens), 0), COALESCE(SUM(total_tokens), 0),
                   COALESCE(SUM(estimated_cost_usd), 0), COALESCE(SUM(priced), 0)
            FROM session_usage_events WHERE timestamp BETWEEN ?1 AND ?2
            GROUP BY model_id
            ORDER BY SUM(estimated_cost_usd) DESC, COUNT(*) DESC
            "#,
        )
        .map_err(|error| format!("准备 Codex 会话模型统计失败: {error}"))?;
    let rows = statement
        .query_map(params![start_at, end_at], |row| {
            Ok(CodexSessionUsageModelStats {
                model_id: row.get(0)?,
                request_count: row.get(1)?,
                input_tokens: row.get(2)?,
                cached_input_tokens: row.get(3)?,
                output_tokens: row.get(4)?,
                total_tokens: row.get(5)?,
                estimated_cost_usd: row.get(6)?,
                priced_request_count: row.get(7)?,
            })
        })
        .map_err(|error| format!("查询 Codex 会话模型统计失败: {error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取 Codex 会话模型统计失败: {error}"))
}

fn query_trends(
    conn: &Connection,
    start_at: i64,
    end_at: i64,
) -> Result<Vec<CodexSessionUsageTrend>, String> {
    let hourly = end_at.saturating_sub(start_at) <= 24 * 60 * 60 * 1000;
    let bucket_format = if hourly { "%Y-%m-%d %H:00" } else { "%Y-%m-%d" };
    let sql = format!(
        r#"
        SELECT strftime('{bucket_format}', timestamp / 1000, 'unixepoch', 'localtime') AS bucket,
               COUNT(*), COALESCE(SUM(input_tokens), 0),
               COALESCE(SUM(cached_input_tokens), 0),
               COALESCE(SUM(output_tokens), 0), COALESCE(SUM(estimated_cost_usd), 0)
        FROM session_usage_events WHERE timestamp BETWEEN ?1 AND ?2
        GROUP BY bucket ORDER BY bucket ASC
        "#
    );
    let mut statement = conn
        .prepare(&sql)
        .map_err(|error| format!("准备 Codex 会话趋势统计失败: {error}"))?;
    let rows = statement
        .query_map(params![start_at, end_at], |row| {
            Ok(CodexSessionUsageTrend {
                bucket: row.get(0)?,
                request_count: row.get(1)?,
                input_tokens: row.get(2)?,
                cached_input_tokens: row.get(3)?,
                output_tokens: row.get(4)?,
                estimated_cost_usd: row.get(5)?,
            })
        })
        .map_err(|error| format!("查询 Codex 会话趋势统计失败: {error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取 Codex 会话趋势统计失败: {error}"))
}

pub fn query_session_usage_summary(
    start_at: i64,
    end_at: i64,
) -> Result<CodexSessionUsageSummary, String> {
    if end_at < start_at {
        return Err("Codex 会话统计结束时间不能早于开始时间".to_string());
    }
    let _guard = SESSION_USAGE_SYNC_LOCK
        .lock()
        .map_err(|_| "Codex 会话统计同步锁已损坏".to_string())?;
    let mut conn = open_session_usage_db()?;
    let sync = sync_session_usage(&mut conn, start_at, end_at);
    Ok(CodexSessionUsageSummary {
        start_at,
        end_at,
        updated_at: now_ms(),
        totals: query_totals(&conn, start_at, end_at)?,
        trends: query_trends(&conn, start_at, end_at)?,
        models: query_models(&conn, start_at, end_at)?,
        files_scanned: sync.files_scanned,
        files_updated: sync.files_updated,
        parse_errors: sync.parse_errors,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn query_session_usage_events(
    start_at: i64,
    end_at: i64,
    page: u64,
    page_size: u64,
    model_query: Option<String>,
) -> Result<CodexSessionUsageEventPage, String> {
    let conn = open_session_usage_db()?;
    let page = page.max(1);
    let page_size = page_size.clamp(1, 100);
    let model_query = model_query.unwrap_or_default().trim().to_ascii_lowercase();
    let model_pattern = format!("%{model_query}%");
    let total: u64 = conn
        .query_row(
            r#"
            SELECT COUNT(*) FROM session_usage_events
            WHERE timestamp BETWEEN ?1 AND ?2
              AND (?3 = '' OR lower(model_id) LIKE ?4)
            "#,
            params![start_at, end_at, model_query, model_pattern],
            |row| row.get(0),
        )
        .map_err(|error| format!("查询 Codex 会话日志数量失败: {error}"))?;
    let total_pages = total.div_ceil(page_size).max(1);
    let page = page.min(total_pages);
    let offset = (page - 1).saturating_mul(page_size);
    let mut statement = conn
        .prepare(
            r#"
            SELECT request_id, thread_id, timestamp, model_id, provider_id,
                   originator, source_label, reasoning_effort, input_tokens,
                   cached_input_tokens, output_tokens, reasoning_output_tokens,
                   total_tokens, estimated_cost_usd, priced
            FROM session_usage_events
            WHERE timestamp BETWEEN ?1 AND ?2
              AND (?3 = '' OR lower(model_id) LIKE ?4)
            ORDER BY timestamp DESC, request_id DESC LIMIT ?5 OFFSET ?6
            "#,
        )
        .map_err(|error| format!("准备 Codex 会话日志查询失败: {error}"))?;
    let rows = statement
        .query_map(
            params![
                start_at,
                end_at,
                model_query,
                model_pattern,
                page_size,
                offset
            ],
            |row| {
                Ok(CodexSessionUsageEvent {
                    request_id: row.get(0)?,
                    thread_id: row.get(1)?,
                    timestamp: row.get(2)?,
                    model_id: row.get(3)?,
                    provider_id: row.get(4)?,
                    originator: row.get(5)?,
                    source_label: row.get(6)?,
                    reasoning_effort: row.get(7)?,
                    input_tokens: row.get(8)?,
                    cached_input_tokens: row.get(9)?,
                    output_tokens: row.get(10)?,
                    reasoning_output_tokens: row.get(11)?,
                    total_tokens: row.get(12)?,
                    estimated_cost_usd: row.get(13)?,
                    priced: row.get(14)?,
                })
            },
        )
        .map_err(|error| format!("查询 Codex 会话日志失败: {error}"))?;
    let events = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取 Codex 会话日志失败: {error}"))?;
    Ok(CodexSessionUsageEventPage {
        events,
        total,
        page,
        page_size,
        total_pages,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        counter_delta, ensure_column, event_key, normalize_model, parse_counters, query_models,
        query_totals, reasoning_effort, sync_session_file, TokenCounters,
    };
    use crate::modules::codex_local_access::CodexSessionPricingResolver;
    use serde_json::json;
    use std::fs;

    #[test]
    fn parses_codex_token_counters() {
        let value = json!({
            "input_tokens": 100,
            "cached_input_tokens": 60,
            "output_tokens": 20,
            "reasoning_output_tokens": 8,
            "total_tokens": 120
        });
        let parsed = parse_counters(Some(&value)).expect("token counters");
        assert_eq!(parsed.input, 100);
        assert_eq!(parsed.cached_input, 60);
        assert_eq!(parsed.output, 20);
        assert_eq!(parsed.reasoning_output, 8);
    }

    #[test]
    fn calculates_saturating_cumulative_delta() {
        let previous = TokenCounters {
            input: 100,
            cached_input: 50,
            output: 10,
            reasoning_output: 4,
            total: 110,
        };
        let current = TokenCounters {
            input: 180,
            cached_input: 90,
            output: 25,
            reasoning_output: 9,
            total: 205,
        };
        let delta = counter_delta(&previous, &current);
        assert_eq!(delta.input, 80);
        assert_eq!(delta.cached_input, 40);
        assert_eq!(delta.output, 15);
        assert_eq!(delta.reasoning_output, 5);
    }

    #[test]
    fn semantic_event_key_deduplicates_replayed_session_events() {
        let usage = TokenCounters {
            input: 100,
            cached_input: 50,
            output: 10,
            reasoning_output: 4,
            total: 110,
        };
        assert_eq!(
            event_key(
                "thread-1",
                1_700_000_000_000,
                "gpt-5.4",
                Some(&usage),
                &usage
            ),
            event_key(
                "thread-1",
                1_700_000_000_000,
                "gpt-5.4",
                Some(&usage),
                &usage
            )
        );
        assert_ne!(
            event_key(
                "thread-1",
                1_700_000_000_000,
                "gpt-5.4",
                Some(&usage),
                &usage
            ),
            event_key(
                "thread-2",
                1_700_000_000_000,
                "gpt-5.4",
                Some(&usage),
                &usage
            )
        );
    }

    #[test]
    fn normalizes_provider_prefixed_model() {
        assert_eq!(normalize_model("OpenAI/GPT-5.4"), "gpt-5.4");
    }

    #[test]
    fn parses_reasoning_effort_variants() {
        assert_eq!(reasoning_effort(&json!({ "effort": "XHIGH" })), "xhigh");
        assert_eq!(
            reasoning_effort(&json!({
                "collaboration_mode": { "settings": { "reasoning_effort": "high" } }
            })),
            "high"
        );
        assert_eq!(reasoning_effort(&json!({})), "");
    }

    #[test]
    fn adds_context_columns_to_legacy_cache_tables() {
        let conn = rusqlite::Connection::open_in_memory().expect("open memory database");
        conn.execute_batch(
            "CREATE TABLE session_usage_events (event_key TEXT PRIMARY KEY);
             CREATE TABLE session_usage_cursors (source_path TEXT PRIMARY KEY);",
        )
        .expect("create legacy schema");
        for (table, column) in [
            ("session_usage_events", "originator"),
            ("session_usage_events", "reasoning_effort"),
            ("session_usage_cursors", "originator"),
            ("session_usage_cursors", "reasoning_effort"),
        ] {
            ensure_column(&conn, table, column, "TEXT NOT NULL DEFAULT ''")
                .expect("add context column");
            ensure_column(&conn, table, column, "TEXT NOT NULL DEFAULT ''")
                .expect("migration is idempotent");
        }
        let event_columns: Vec<String> = conn
            .prepare("PRAGMA table_info(session_usage_events)")
            .expect("prepare event table info")
            .query_map([], |row| row.get(1))
            .expect("query event table info")
            .collect::<Result<_, _>>()
            .expect("collect event columns");
        assert!(event_columns.contains(&"originator".to_string()));
        assert!(event_columns.contains(&"reasoning_effort".to_string()));
    }

    #[test]
    fn imports_jsonl_and_skips_repeated_total_snapshot() {
        let dir =
            std::env::temp_dir().join(format!("cockpit-session-usage-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("create temp dir");
        let db_path = dir.join("usage.sqlite");
        let session_path =
            dir.join("rollout-2026-08-12T10-00-00-11111111-1111-4111-8111-111111111111.jsonl");
        let lines = [
            json!({"timestamp":"2026-08-12T02:00:00Z","type":"session_meta","payload":{"id":"11111111-1111-4111-8111-111111111111","model_provider":"relay","originator":"Codex Desktop","source":"vscode"}}),
            json!({"timestamp":"2026-08-12T02:00:01Z","type":"turn_context","payload":{"model":"gpt-5.4","effort":"xhigh"}}),
            json!({"timestamp":"2026-08-12T02:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":60,"output_tokens":20,"reasoning_output_tokens":5,"total_tokens":120},"last_token_usage":{"input_tokens":100,"cached_input_tokens":60,"output_tokens":20,"reasoning_output_tokens":5,"total_tokens":120}}}}),
            json!({"timestamp":"2026-08-12T02:00:03Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":60,"output_tokens":20,"reasoning_output_tokens":5,"total_tokens":120},"last_token_usage":{"input_tokens":100,"cached_input_tokens":60,"output_tokens":20,"reasoning_output_tokens":5,"total_tokens":120}}}}),
        ];
        let content = format!(
            "{}\n",
            lines
                .iter()
                .map(|line| serde_json::to_string(line).expect("serialize line"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        fs::write(&session_path, content).expect("write session");

        let mut conn = rusqlite::Connection::open(&db_path).expect("open db");
        conn.execute_batch(
            r#"
            CREATE TABLE session_usage_events (
                event_key TEXT PRIMARY KEY, request_id TEXT NOT NULL, thread_id TEXT NOT NULL,
                source_path TEXT NOT NULL, timestamp INTEGER NOT NULL, model_id TEXT NOT NULL,
                provider_id TEXT NOT NULL, originator TEXT NOT NULL, source_label TEXT NOT NULL,
                reasoning_effort TEXT NOT NULL, input_tokens INTEGER NOT NULL,
                cached_input_tokens INTEGER NOT NULL, output_tokens INTEGER NOT NULL,
                reasoning_output_tokens INTEGER NOT NULL, total_tokens INTEGER NOT NULL,
                estimated_cost_usd REAL NOT NULL, priced INTEGER NOT NULL, pricing_version INTEGER NOT NULL,
                input_usd_per_million REAL NOT NULL, cached_input_usd_per_million REAL NOT NULL,
                output_usd_per_million REAL NOT NULL
            );
            CREATE TABLE session_usage_cursors (
                source_path TEXT PRIMARY KEY, byte_offset INTEGER NOT NULL, file_size INTEGER NOT NULL,
                modified_ms INTEGER NOT NULL, current_model TEXT NOT NULL, thread_id TEXT NOT NULL,
                provider_id TEXT NOT NULL, originator TEXT NOT NULL, source_label TEXT NOT NULL,
                reasoning_effort TEXT NOT NULL, total_input INTEGER NOT NULL,
                total_cached_input INTEGER NOT NULL, total_output INTEGER NOT NULL,
                total_reasoning_output INTEGER NOT NULL, total_tokens INTEGER NOT NULL
            );
            "#,
        )
        .expect("create schema");
        sync_session_file(
            &mut conn,
            &session_path,
            &CodexSessionPricingResolver::load(),
        )
        .expect("sync session");
        let totals = query_totals(&conn, 0, i64::MAX).expect("query totals");
        let models = query_models(&conn, 0, i64::MAX).expect("query models");
        assert_eq!(totals.request_count, 1);
        assert_eq!(totals.input_tokens, 100);
        assert_eq!(totals.cached_input_tokens, 60);
        assert_eq!(models[0].model_id, "gpt-5.4");
        let (originator, source_label, effort): (String, String, String) = conn
            .query_row(
                "SELECT originator, source_label, reasoning_effort FROM session_usage_events",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("query imported context fields");
        assert_eq!(originator, "Codex Desktop");
        assert_eq!(source_label, "vscode");
        assert_eq!(effort, "xhigh");

        let _ = fs::remove_dir_all(dir);
    }
}
