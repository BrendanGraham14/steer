use async_trait::async_trait;
use chrono::Utc;
use serde_json;
use sqlx::{
    Row,
    sqlite::{
        SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
    },
};
use std::path::Path;
use std::str::FromStr;
use uuid::Uuid;

use crate::app::Message;
use tools::ToolCall;
use crate::events::StreamEvent;
use crate::session::{
    Session, SessionConfig, SessionFilter, SessionInfo, SessionOrderBy, SessionState,
    SessionStatus, SessionStore, SessionStoreError, ToolApprovalPolicy, ToolCallState,
    ToolCallStatus, ToolCallUpdate, ToolResult,
};
use crate::session::state::ToolVisibility;

/// SQLite implementation of SessionStore
pub struct SqliteSessionStore {
    pool: SqlitePool,
}

impl SqliteSessionStore {
    /// Create a new SQLite session store
    pub async fn new(path: &Path) -> Result<Self, SessionStoreError> {
        // Create parent directory if it doesn't exist
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                SessionStoreError::connection(format!("Failed to create directory: {}", e))
            })?;
        }

        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
            .map_err(|e| SessionStoreError::connection(format!("Invalid SQLite path: {}", e)))?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(1) // Single connection for local use
            .connect_with(options)
            .await
            .map_err(|e| {
                SessionStoreError::connection(format!("Failed to connect to SQLite: {}", e))
            })?;

        // Run migrations
        sqlx::migrate!("../migrations/sqlite")
            .run(&pool)
            .await
            .map_err(|e| SessionStoreError::Migration {
                message: format!("Failed to run migrations: {}", e),
            })?;

        Ok(Self { pool })
    }

    /// Parse tool approval policy from database format
    fn parse_tool_policy(
        policy_type: &str,
        pre_approved_json: &str,
    ) -> Result<ToolApprovalPolicy, SessionStoreError> {
        let pre_approved: Vec<String> = serde_json::from_str(pre_approved_json).map_err(|e| {
            SessionStoreError::serialization(format!("Invalid pre_approved_tools: {}", e))
        })?;

        match policy_type {
            "always_ask" => Ok(ToolApprovalPolicy::AlwaysAsk),
            "pre_approved" => Ok(ToolApprovalPolicy::PreApproved {
                tools: pre_approved.into_iter().collect(),
            }),
            "mixed" => Ok(ToolApprovalPolicy::Mixed {
                pre_approved: pre_approved.into_iter().collect(),
                ask_for_others: true,
            }),
            _ => Err(SessionStoreError::validation(format!(
                "Invalid tool policy type: {}",
                policy_type
            ))),
        }
    }

    /// Convert tool approval policy to database format
    fn serialize_tool_policy(policy: &ToolApprovalPolicy) -> (String, String) {
        match policy {
            ToolApprovalPolicy::AlwaysAsk => ("always_ask".to_string(), "[]".to_string()),
            ToolApprovalPolicy::PreApproved { tools } => {
                let tools_vec: Vec<String> = tools.iter().cloned().collect();
                (
                    "pre_approved".to_string(),
                    serde_json::to_string(&tools_vec).unwrap(),
                )
            }
            ToolApprovalPolicy::Mixed { pre_approved, .. } => {
                let tools_vec: Vec<String> = pre_approved.iter().cloned().collect();
                (
                    "mixed".to_string(),
                    serde_json::to_string(&tools_vec).unwrap(),
                )
            }
        }
    }
}

#[async_trait]
impl SessionStore for SqliteSessionStore {
    async fn create_session(&self, config: SessionConfig) -> Result<Session, SessionStoreError> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let (policy_type, pre_approved_json) = Self::serialize_tool_policy(&config.tool_config.approval_policy);
        let metadata_json = serde_json::to_string(&config.metadata).map_err(|e| {
            SessionStoreError::serialization(format!("Failed to serialize metadata: {}", e))
        })?;
        let tool_config_json = serde_json::to_string(&config.tool_config).map_err(|e| {
            SessionStoreError::serialization(format!("Failed to serialize tool_config: {}", e))
        })?;
        let workspace_config_json = serde_json::to_string(&config.workspace).map_err(|e| {
            SessionStoreError::serialization(format!("Failed to serialize workspace_config: {}", e))
        })?;

        sqlx::query(
            r#"
            INSERT INTO sessions (id, created_at, updated_at, status, metadata,
                                  tool_policy_type, pre_approved_tools, tool_config, workspace_config)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
        )
        .bind(&id)
        .bind(now)
        .bind(now)
        .bind("inactive") // New sessions start as inactive
        .bind(&metadata_json)
        .bind(&policy_type)
        .bind(&pre_approved_json)
        .bind(&tool_config_json)
        .bind(&workspace_config_json)
        .execute(&self.pool)
        .await
        .map_err(|e| SessionStoreError::database(format!("Failed to create session: {}", e)))?;

        Ok(Session {
            id: id.clone(),
            created_at: now,
            updated_at: now,
            config,
            state: SessionState::default(),
        })
    }

    async fn get_session(&self, session_id: &str) -> Result<Option<Session>, SessionStoreError> {
        let row = sqlx::query(
            r#"
            SELECT id, created_at, updated_at, metadata,
                   tool_policy_type, pre_approved_tools, tool_config, workspace_config
            FROM sessions
            WHERE id = ?1
            "#,
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| SessionStoreError::database(format!("Failed to get session: {}", e)))?;

        let Some(row) = row else {
            return Ok(None);
        };

        let approval_policy = Self::parse_tool_policy(
            &row.get::<String, _>("tool_policy_type"),
            &row.get::<String, _>("pre_approved_tools"),
        )?;

        let metadata: std::collections::HashMap<String, String> =
            serde_json::from_str(&row.get::<String, _>("metadata")).map_err(|e| {
                SessionStoreError::serialization(format!("Invalid metadata: {}", e))
            })?;

        let tool_config = serde_json::from_str(&row.get::<String, _>("tool_config"))
            .map_err(|e| SessionStoreError::serialization(format!("Invalid tool_config: {}", e)))?;
        let workspace_config = serde_json::from_str(&row.get::<String, _>("workspace_config"))
            .map_err(|e| {
                SessionStoreError::serialization(format!("Invalid workspace_config: {}", e))
            })?;

        let mut tool_config: crate::session::SessionToolConfig = tool_config;
        tool_config.approval_policy = approval_policy;
        
        let config = SessionConfig {
            workspace: workspace_config,
            tool_config,
            metadata,
        };

        // Load messages
        let messages = self.get_messages(session_id, None).await?;

        // Load tool calls
        let tool_calls_rows = sqlx::query(
            r#"
            SELECT id, tool_name, parameters, status, result, error, started_at, completed_at
            FROM tool_calls
            WHERE session_id = ?1
            "#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| SessionStoreError::database(format!("Failed to load tool calls: {}", e)))?;

        let mut tool_calls = std::collections::HashMap::new();
        for row in tool_calls_rows {
            let id: String = row.get("id");
            let status_str: String = row.get("status");
            let error: Option<String> = row.get("error");

            let status = match status_str.as_str() {
                "pending" => ToolCallStatus::PendingApproval,
                "approved" => ToolCallStatus::Approved,
                "denied" => ToolCallStatus::Denied,
                "executing" => ToolCallStatus::Executing,
                "completed" => ToolCallStatus::Completed,
                "failed" => ToolCallStatus::Failed {
                    error: error.unwrap_or_else(|| "Unknown error".to_string()),
                },
                _ => {
                    return Err(SessionStoreError::validation(format!(
                        "Invalid tool call status: {}",
                        status_str
                    )));
                }
            };

            let tool_call = ToolCall {
                id: id.clone(),
                name: row.get("tool_name"),
                parameters: serde_json::from_str(&row.get::<String, _>("parameters")).map_err(
                    |e| SessionStoreError::serialization(format!("Invalid tool parameters: {}", e)),
                )?,
            };

            let result: Option<String> = row.get("result");
            let tool_result = result.map(|r| ToolResult {
                output: r,
                success: true,
                execution_time_ms: 0,
                metadata: std::collections::HashMap::new(),
            });

            let state = ToolCallState {
                tool_call,
                status,
                started_at: row.get("started_at"),
                completed_at: row.get("completed_at"),
                result: tool_result,
            };

            tool_calls.insert(id, state);
        }

        // Get the latest event sequence number
        let last_sequence: Option<i64> =
            sqlx::query_scalar("SELECT MAX(sequence_num) FROM events WHERE session_id = ?1")
                .bind(session_id)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| {
                    SessionStoreError::database(format!("Failed to get last event sequence: {}", e))
                })?;

        let state = SessionState {
            messages,
            tool_calls,
            approved_tools: Default::default(), // TODO: Track approved tools separately if needed
            last_event_sequence: last_sequence.unwrap_or(0) as u64,
            metadata: Default::default(),
        };

        Ok(Some(Session {
            id: row.get("id"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
            config,
            state,
        }))
    }

    async fn update_session(&self, session: &Session) -> Result<(), SessionStoreError> {
        let metadata_json = serde_json::to_string(&session.config.metadata).map_err(|e| {
            SessionStoreError::serialization(format!("Failed to serialize metadata: {}", e))
        })?;
        let tool_config_json = serde_json::to_string(&session.config.tool_config).map_err(|e| {
            SessionStoreError::serialization(format!("Failed to serialize tool_config: {}", e))
        })?;
        let workspace_config_json =
            serde_json::to_string(&session.config.workspace).map_err(|e| {
                SessionStoreError::serialization(format!(
                    "Failed to serialize workspace_config: {}",
                    e
                ))
            })?;
        let (policy_type, pre_approved_json) =
            Self::serialize_tool_policy(&session.config.tool_config.approval_policy);

        sqlx::query(
            r#"
            UPDATE sessions
            SET updated_at = ?2, metadata = ?3,
                tool_policy_type = ?4, pre_approved_tools = ?5, tool_config = ?6, workspace_config = ?7
            WHERE id = ?1
            "#,
        )
        .bind(&session.id)
        .bind(Utc::now())
        .bind(&metadata_json)
        .bind(&policy_type)
        .bind(&pre_approved_json)
        .bind(&tool_config_json)
        .bind(&workspace_config_json)
        .execute(&self.pool)
        .await
        .map_err(|e| SessionStoreError::database(format!("Failed to update session: {}", e)))?;

        Ok(())
    }

    async fn delete_session(&self, session_id: &str) -> Result<(), SessionStoreError> {
        sqlx::query("DELETE FROM sessions WHERE id = ?1")
            .bind(session_id)
            .execute(&self.pool)
            .await
            .map_err(|e| SessionStoreError::database(format!("Failed to delete session: {}", e)))?;

        Ok(())
    }

    async fn list_sessions(
        &self,
        filter: SessionFilter,
    ) -> Result<Vec<SessionInfo>, SessionStoreError> {
        let mut query = String::from(
            r#"
            SELECT s.id, s.created_at, s.updated_at, s.status, s.metadata,
                   (SELECT e.event_data
                    FROM events e
                    WHERE e.session_id = s.id
                      AND e.event_type IN ('message_complete', 'tool_call_started', 'tool_call_completed', 'tool_call_failed')
                    ORDER BY e.sequence_num DESC
                    LIMIT 1) as last_event_data
            FROM sessions s
            WHERE 1=1
            "#,
        );
        let mut bindings: Vec<String> = Vec::new();

        // Apply filters
        if let Some(created_after) = filter.created_after {
            query.push_str(&format!(" AND s.created_at >= ?{}", bindings.len() + 1));
            bindings.push(created_after.to_rfc3339());
        }
        if let Some(created_before) = filter.created_before {
            query.push_str(&format!(" AND s.created_at <= ?{}", bindings.len() + 1));
            bindings.push(created_before.to_rfc3339());
        }
        if let Some(updated_after) = filter.updated_after {
            query.push_str(&format!(" AND s.updated_at >= ?{}", bindings.len() + 1));
            bindings.push(updated_after.to_rfc3339());
        }
        if let Some(updated_before) = filter.updated_before {
            query.push_str(&format!(" AND s.updated_at <= ?{}", bindings.len() + 1));
            bindings.push(updated_before.to_rfc3339());
        }
        if let Some(status) = filter.status_filter {
            let status_str = match status {
                SessionStatus::Active => "active",
                SessionStatus::Inactive => "inactive",
            };
            query.push_str(&format!(" AND s.status = ?{}", bindings.len() + 1));
            bindings.push(status_str.to_string());
        }

        // Add ordering
        let order_column = match filter.order_by {
            SessionOrderBy::CreatedAt => "s.created_at",
            SessionOrderBy::UpdatedAt => "s.updated_at",
            SessionOrderBy::MessageCount => {
                // For message count, we'll need a subquery
                query = r#"
                    SELECT s.id, s.created_at, s.updated_at, s.status, s.metadata,
                           (SELECT e.event_data
                            FROM events e
                            WHERE e.session_id = s.id
                              AND e.event_type IN ('message_complete', 'tool_call_started', 'tool_call_completed', 'tool_call_failed')
                            ORDER BY e.sequence_num DESC
                            LIMIT 1) as last_event_data,
                           (SELECT COUNT(*) FROM messages WHERE session_id = s.id) as message_count
                    FROM sessions s
                    WHERE 1=1
                    "#.to_string();
                "message_count"
            }
        };

        let order_direction = match filter.order_direction {
            crate::session::OrderDirection::Ascending => "ASC",
            crate::session::OrderDirection::Descending => "DESC",
        };

        query.push_str(&format!(" ORDER BY {} {}", order_column, order_direction));

        // Add pagination
        if let Some(limit) = filter.limit {
            query.push_str(&format!(" LIMIT {}", limit));
        }
        if let Some(offset) = filter.offset {
            query.push_str(&format!(" OFFSET {}", offset));
        }

        // Execute query with dynamic bindings
        let mut q = sqlx::query(&query);
        for binding in bindings {
            q = q.bind(binding);
        }

        let rows = q
            .fetch_all(&self.pool)
            .await
            .map_err(|e| SessionStoreError::database(format!("Failed to list sessions: {}", e)))?;

        let mut sessions = Vec::new();
        for row in rows {
            let metadata: std::collections::HashMap<String, String> =
                serde_json::from_str(&row.get::<String, _>("metadata")).map_err(|e| {
                    SessionStoreError::serialization(format!("Invalid metadata: {}", e))
                })?;

            // Count messages for this session (if not already done in query)
            let message_count: i64 = if matches!(filter.order_by, SessionOrderBy::MessageCount) {
                row.get("message_count")
            } else {
                sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE session_id = ?1")
                    .bind(row.get::<String, _>("id"))
                    .fetch_one(&self.pool)
                    .await
                    .map_err(|e| {
                        SessionStoreError::database(format!("Failed to count messages: {}", e))
                    })?
            };

            // Extract last model from event data
            let last_model =
                if let Some(event_json) = row.get::<Option<String>, _>("last_event_data") {
                    let event: StreamEvent = serde_json::from_str(&event_json).map_err(|e| {
                        SessionStoreError::serialization(format!("Invalid event data: {}", e))
                    })?;

                    match event {
                        StreamEvent::MessageComplete { model, .. } => Some(model),
                        StreamEvent::ToolCallStarted { model, .. } => Some(model),
                        StreamEvent::ToolCallCompleted { model, .. } => Some(model),
                        StreamEvent::ToolCallFailed { model, .. } => Some(model),
                        _ => None,
                    }
                } else {
                    None
                };

            sessions.push(SessionInfo {
                id: row.get("id"),
                created_at: row.get("created_at"),
                updated_at: row.get("updated_at"),
                last_model,
                message_count: message_count as usize,
                metadata,
            });
        }

        Ok(sessions)
    }

    async fn append_message(
        &self,
        session_id: &str,
        message: &Message,
    ) -> Result<(), SessionStoreError> {
        let id = &message.id;

        // Get the next sequence number
        let next_seq: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence_num), -1) + 1 FROM messages WHERE session_id = ?1",
        )
        .bind(session_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| SessionStoreError::database(format!("Failed to get next sequence: {}", e)))?;

        // Serialize the conversation message content blocks directly
        let content_json = serde_json::to_string(&message.content_blocks).map_err(|e| {
            SessionStoreError::serialization(format!("Failed to serialize message content: {}", e))
        })?;

        sqlx::query(
            r#"
            INSERT INTO messages (id, session_id, sequence_num, role, content, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
        )
        .bind(&id)
        .bind(session_id)
        .bind(next_seq)
        .bind(match message.role {
            crate::app::conversation::Role::User => "user",
            crate::app::conversation::Role::Assistant => "assistant", 
            crate::app::conversation::Role::Tool => "tool",
        })
        .bind(&content_json)
        .bind(Utc::now())
        .execute(&self.pool)
        .await
        .map_err(|e| SessionStoreError::database(format!("Failed to append message: {}", e)))?;

        Ok(())
    }

    async fn get_messages(
        &self,
        session_id: &str,
        after_sequence: Option<u32>,
    ) -> Result<Vec<Message>, SessionStoreError> {
        let query = if let Some(seq) = after_sequence {
            sqlx::query(
                r#"
                SELECT id, sequence_num, role, content, created_at
                FROM messages
                WHERE session_id = ?1 AND sequence_num > ?2
                ORDER BY sequence_num ASC
                "#,
            )
            .bind(session_id)
            .bind(seq as i64)
        } else {
            sqlx::query(
                r#"
                SELECT id, sequence_num, role, content, created_at
                FROM messages
                WHERE session_id = ?1
                ORDER BY sequence_num ASC
                "#,
            )
            .bind(session_id)
        };

        let rows = query
            .fetch_all(&self.pool)
            .await
            .map_err(|e| SessionStoreError::database(format!("Failed to get messages: {}", e)))?;

        let mut messages = Vec::new();
        for row in rows {
            let content_blocks: Vec<crate::app::conversation::MessageContentBlock> = 
                serde_json::from_str(&row.get::<String, _>("content"))
                    .map_err(|e| {
                        SessionStoreError::serialization(format!("Invalid message content: {}", e))
                    })?;

            let message = Message {
                id: row.get("id"),
                role: match row.get::<String, _>("role").as_str() {
                    "user" => crate::app::conversation::Role::User,
                    "assistant" => crate::app::conversation::Role::Assistant,
                    "tool" => crate::app::conversation::Role::Tool,
                    role => {
                        return Err(SessionStoreError::validation(format!(
                            "Invalid role: {}",
                            role
                        )));
                    }
                },
                content_blocks,
                timestamp: row.get::<chrono::DateTime<chrono::Utc>, _>("created_at").timestamp() as u64,
            };
            
            messages.push(message);
        }

        Ok(messages)
    }

    async fn create_tool_call(
        &self,
        session_id: &str,
        tool_call: &ToolCall,
    ) -> Result<(), SessionStoreError> {
        let parameters_json = serde_json::to_string(&tool_call.parameters).map_err(|e| {
            SessionStoreError::serialization(format!("Failed to serialize parameters: {}", e))
        })?;

        sqlx::query(
            r#"
            INSERT INTO tool_calls (id, session_id, tool_name, parameters, status)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
        )
        .bind(&tool_call.id)
        .bind(session_id)
        .bind(&tool_call.name)
        .bind(&parameters_json)
        .bind("pending")
        .execute(&self.pool)
        .await
        .map_err(|e| SessionStoreError::database(format!("Failed to create tool call: {}", e)))?;

        Ok(())
    }

    async fn update_tool_call(
        &self,
        tool_call_id: &str,
        update: ToolCallUpdate,
    ) -> Result<(), SessionStoreError> {
        let mut query = String::from("UPDATE tool_calls SET ");
        let mut updates = Vec::new();
        let mut bindings: Vec<String> = Vec::new();

        if let Some(status) = update.status {
            let status_str = match &status {
                ToolCallStatus::PendingApproval => "pending",
                ToolCallStatus::Approved => "approved",
                ToolCallStatus::Denied => "denied",
                ToolCallStatus::Executing => "executing",
                ToolCallStatus::Completed => "completed",
                ToolCallStatus::Failed { .. } => "failed",
            };
            updates.push(format!("status = ?{}", bindings.len() + 1));
            bindings.push(status_str.to_string());

            // Update timestamps based on status
            match status {
                ToolCallStatus::Executing => {
                    updates.push(format!("started_at = ?{}", bindings.len() + 1));
                    bindings.push(Utc::now().to_rfc3339());
                }
                ToolCallStatus::Completed | ToolCallStatus::Failed { .. } => {
                    updates.push(format!("completed_at = ?{}", bindings.len() + 1));
                    bindings.push(Utc::now().to_rfc3339());
                }
                _ => {}
            }
        }

        if let Some(result) = update.result {
            updates.push(format!("result = ?{}", bindings.len() + 1));
            bindings.push(result.output);
        }

        if let Some(error) = update.error {
            updates.push(format!("error = ?{}", bindings.len() + 1));
            bindings.push(error);
        }

        if updates.is_empty() {
            return Ok(());
        }

        query.push_str(&updates.join(", "));
        query.push_str(&format!(" WHERE id = ?{}", bindings.len() + 1));
        bindings.push(tool_call_id.to_string());

        // Execute with dynamic bindings
        let mut q = sqlx::query(&query);
        for binding in bindings {
            q = q.bind(binding);
        }

        q.execute(&self.pool).await.map_err(|e| {
            SessionStoreError::database(format!("Failed to update tool call: {}", e))
        })?;

        Ok(())
    }

    async fn get_pending_tool_calls(
        &self,
        session_id: &str,
    ) -> Result<Vec<ToolCall>, SessionStoreError> {
        let rows = sqlx::query(
            r#"
            SELECT id, tool_name, parameters
            FROM tool_calls
            WHERE session_id = ?1 AND status = 'pending'
            ORDER BY id ASC
            "#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            SessionStoreError::database(format!("Failed to get pending tool calls: {}", e))
        })?;

        let mut tool_calls = Vec::new();
        for row in rows {
            let parameters: serde_json::Value =
                serde_json::from_str(&row.get::<String, _>("parameters")).map_err(|e| {
                    SessionStoreError::serialization(format!("Invalid parameters: {}", e))
                })?;

            tool_calls.push(ToolCall {
                id: row.get("id"),
                name: row.get("tool_name"),
                parameters,
            });
        }

        Ok(tool_calls)
    }

    async fn append_event(
        &self,
        session_id: &str,
        event: &StreamEvent,
    ) -> Result<u64, SessionStoreError> {
        let event_type = match event {
            StreamEvent::MessagePart { .. } => "message_part",
            StreamEvent::MessageComplete { .. } => "message_complete",
            StreamEvent::ToolCallStarted { .. } => "tool_call_started",
            StreamEvent::ToolCallCompleted { .. } => "tool_call_completed",
            StreamEvent::ToolCallFailed { .. } => "tool_call_failed",
            StreamEvent::ToolApprovalRequired { .. } => "tool_approval_required",
            StreamEvent::SessionCreated { .. } => "session_created",
            StreamEvent::SessionResumed { .. } => "session_resumed",
            StreamEvent::SessionSaved { .. } => "session_saved",
            StreamEvent::OperationStarted { .. } => "operation_started",
            StreamEvent::OperationCompleted { .. } => "operation_completed",
            StreamEvent::OperationCancelled { .. } => "operation_cancelled",
            StreamEvent::Error { .. } => "error",
        };

        let event_data = serde_json::to_string(event).map_err(|e| {
            SessionStoreError::serialization(format!("Failed to serialize event: {}", e))
        })?;

        // Get the next sequence number
        let next_seq: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence_num), -1) + 1 FROM events WHERE session_id = ?1",
        )
        .bind(session_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| SessionStoreError::database(format!("Failed to get next sequence: {}", e)))?;

        sqlx::query(
            r#"
            INSERT INTO events (session_id, sequence_num, event_type, event_data, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
        )
        .bind(session_id)
        .bind(next_seq)
        .bind(event_type)
        .bind(&event_data)
        .bind(Utc::now())
        .execute(&self.pool)
        .await
        .map_err(|e| SessionStoreError::database(format!("Failed to append event: {}", e)))?;

        Ok(next_seq as u64)
    }

    async fn get_events(
        &self,
        session_id: &str,
        after_sequence: u64,
        limit: Option<u32>,
    ) -> Result<Vec<(u64, StreamEvent)>, SessionStoreError> {
        let query = if let Some(limit) = limit {
            sqlx::query(
                r#"
                SELECT sequence_num, event_data
                FROM events
                WHERE session_id = ?1 AND sequence_num > ?2
                ORDER BY sequence_num ASC
                LIMIT ?3
                "#,
            )
            .bind(session_id)
            .bind(after_sequence as i64)
            .bind(limit as i64)
        } else {
            sqlx::query(
                r#"
                SELECT sequence_num, event_data
                FROM events
                WHERE session_id = ?1 AND sequence_num > ?2
                ORDER BY sequence_num ASC
                "#,
            )
            .bind(session_id)
            .bind(after_sequence as i64)
        };

        let rows = query
            .fetch_all(&self.pool)
            .await
            .map_err(|e| SessionStoreError::database(format!("Failed to get events: {}", e)))?;

        let mut events = Vec::new();
        for row in rows {
            let seq: i64 = row.get("sequence_num");
            let event: StreamEvent = serde_json::from_str(&row.get::<String, _>("event_data"))
                .map_err(|e| {
                    SessionStoreError::serialization(format!("Invalid event data: {}", e))
                })?;

            events.push((seq as u64, event));
        }

        Ok(events)
    }

    async fn delete_events_before(
        &self,
        session_id: &str,
        before_sequence: u64,
    ) -> Result<u64, SessionStoreError> {
        let result = sqlx::query("DELETE FROM events WHERE session_id = ?1 AND sequence_num < ?2")
            .bind(session_id)
            .bind(before_sequence as i64)
            .execute(&self.pool)
            .await
            .map_err(|e| SessionStoreError::database(format!("Failed to delete events: {}", e)))?;

        Ok(result.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use crate::api::{Model, ToolCall};
    use crate::events::SessionMetadata;
    use crate::session::state::WorkspaceConfig;
    use crate::app::conversation::{MessageContentBlock, Role};

    use super::*;
    use tempfile::TempDir;

    async fn create_test_store() -> (SqliteSessionStore, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let store = SqliteSessionStore::new(&db_path).await.unwrap();
        (store, temp_dir)
    }

    fn create_test_session_config() -> SessionConfig {
        let mut tool_config = crate::session::SessionToolConfig::default();
        tool_config.approval_policy = ToolApprovalPolicy::AlwaysAsk;
        tool_config.visibility = ToolVisibility::All;
        
        SessionConfig {
            workspace: WorkspaceConfig::default(),
            tool_config,
            metadata: std::collections::HashMap::new(),
        }
    }

    #[tokio::test]
    async fn test_create_and_get_session() {
        let (store, _temp) = create_test_store().await;

        let mut tool_config = crate::session::SessionToolConfig::default();
        tool_config.approval_policy = ToolApprovalPolicy::AlwaysAsk;
        
        let config = SessionConfig {
            workspace: WorkspaceConfig::default(),
            tool_config,
            metadata: Default::default(),
        };

        let session = store.create_session(config.clone()).await.unwrap();
        assert!(!session.id.is_empty());

        let fetched_session = store.get_session(&session.id).await.unwrap().unwrap();
        assert_eq!(session.id, fetched_session.id);
        assert!(matches!(
            fetched_session.config.tool_config.approval_policy,
            ToolApprovalPolicy::AlwaysAsk
        ));
        assert!(matches!(
            fetched_session.config.workspace,
            WorkspaceConfig::Local
        ));
    }

    #[tokio::test]
    async fn test_message_operations() {
        let (store, _temp) = create_test_store().await;

        let config = create_test_session_config();
        let session = store.create_session(config).await.unwrap();

        let message = Message {
            id: "msg1".to_string(),
            role: Role::User,
            content_blocks: vec![MessageContentBlock::Text("Hello".to_string())],
            timestamp: 123456789,
        };

        store.append_message(&session.id, &message).await.unwrap();

        let messages = store.get_messages(&session.id, None).await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, Role::User);
    }

    #[tokio::test]
    async fn test_tool_call_operations() {
        let (store, _temp) = create_test_store().await;

        let config = create_test_session_config();
        let session = store.create_session(config).await.unwrap();

        let tool_call = ToolCall {
            id: "tc1".to_string(),
            name: "test_tool".to_string(),
            parameters: serde_json::json!({"param": "value"}),
        };

        store
            .create_tool_call(&session.id, &tool_call)
            .await
            .unwrap();

        let pending = store.get_pending_tool_calls(&session.id).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].name, "test_tool");

        let update = ToolCallUpdate::set_status(ToolCallStatus::Completed);
        store.update_tool_call(&tool_call.id, update).await.unwrap();

        let pending_after = store.get_pending_tool_calls(&session.id).await.unwrap();
        assert_eq!(pending_after.len(), 0);
    }

    #[tokio::test]
    async fn test_event_streaming() {
        let (store, _temp) = create_test_store().await;

        let config = create_test_session_config();
        let session = store.create_session(config).await.unwrap();

        let event = StreamEvent::SessionCreated {
            session_id: session.id.clone(),
            metadata: SessionMetadata {
                model: Model::Claude3_5Sonnet20241022,
                created_at: session.created_at,
                metadata: session.config.metadata,
            },
        };

        let seq = store.append_event(&session.id, &event).await.unwrap();
        assert_eq!(seq, 0);

        // Get events after sequence 0 (should be empty since we only have sequence 0)
        let events = store.get_events(&session.id, 0, None).await.unwrap();
        assert_eq!(events.len(), 0);

        // Get all events including sequence 0 by asking for events after -1
        let all_events = store.get_events(&session.id, u64::MAX, None).await.unwrap();
        assert_eq!(all_events.len(), 1);
        assert_eq!(all_events[0].0, 0);
    }

    #[tokio::test]
    async fn test_session_listing() {
        let (store, _temp) = create_test_store().await;

        // Create multiple sessions
        for i in 0..3 {
            let mut config = create_test_session_config();
            config.metadata.insert("index".to_string(), i.to_string());
            store.create_session(config).await.unwrap();
        }

        let filter = SessionFilter {
            limit: Some(2),
            order_by: SessionOrderBy::CreatedAt,
            ..Default::default()
        };

        let sessions = store.list_sessions(filter).await.unwrap();
        assert_eq!(sessions.len(), 2);
    }

    #[tokio::test]
    async fn test_last_model_tracking() {
        let (store, _temp) = create_test_store().await;

        let config = create_test_session_config();
        let session = store.create_session(config).await.unwrap();

        // Initially, no events means no last model
        let sessions = store.list_sessions(SessionFilter::default()).await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].last_model, None);

        // Add a MessageComplete event with Claude model
        let claude_model = Model::Claude3_5Sonnet20241022;
        let message_event = StreamEvent::MessageComplete {
            message: Message {
                id: "msg1".to_string(),
                role: Role::Assistant,
                content_blocks: vec![MessageContentBlock::Text("Hello from Claude".to_string())],
                timestamp: 123456789,
            },
            usage: None,
            metadata: std::collections::HashMap::new(),
            model: claude_model,
        };
        store
            .append_event(&session.id, &message_event)
            .await
            .unwrap();

        // Check that last_model is now Claude
        let sessions = store.list_sessions(SessionFilter::default()).await.unwrap();
        assert_eq!(sessions[0].last_model, Some(claude_model));

        // Add a ToolCallStarted event with GPT model (more recent)
        let gpt_model = Model::Gpt4_1_20250414;
        let tool_event = StreamEvent::ToolCallStarted {
            tool_call: ToolCall {
                id: "tool1".to_string(),
                name: "test_tool".to_string(),
                parameters: serde_json::json!({"param": "value"}),
            },
            metadata: std::collections::HashMap::new(),
            model: gpt_model,
        };
        store.append_event(&session.id, &tool_event).await.unwrap();

        // Check that last_model is now GPT (the most recent)
        let sessions = store.list_sessions(SessionFilter::default()).await.unwrap();
        assert_eq!(sessions[0].last_model, Some(gpt_model));

        // Add an event without a model field (shouldn't change last_model)
        let session_event = StreamEvent::SessionSaved {
            session_id: session.id.clone(),
        };
        store
            .append_event(&session.id, &session_event)
            .await
            .unwrap();

        // Check that last_model is still GPT
        let sessions = store.list_sessions(SessionFilter::default()).await.unwrap();
        assert_eq!(sessions[0].last_model, Some(gpt_model));
    }
}
