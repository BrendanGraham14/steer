-- Migration: Remove legacy tool_policy_type and pre_approved_tools columns
-- These columns are now redundant - approval policy is stored in tool_config JSON
-- 
-- NOTE: This migration file is for a legacy schema. The current event store
-- (SqliteEventStore) uses domain_sessions/domain_events tables with inline migrations.
-- This file exists for backwards compatibility with older database schemas.

-- SQLite doesn't support DROP COLUMN directly, so we recreate the table
CREATE TABLE sessions_new (
    id TEXT PRIMARY KEY,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    status TEXT NOT NULL CHECK (status IN ('active', 'inactive')),
    metadata TEXT NOT NULL DEFAULT '{}',
    tool_config TEXT NOT NULL DEFAULT '{}',
    workspace_config TEXT NOT NULL DEFAULT '{}',
    system_prompt TEXT,
    active_message_id TEXT
);

-- Copy data from old table (excluding removed columns)
INSERT INTO sessions_new (id, created_at, updated_at, status, metadata, tool_config, workspace_config, system_prompt, active_message_id)
SELECT id, created_at, updated_at, status, metadata, tool_config, workspace_config, system_prompt, active_message_id
FROM sessions;

-- Drop old table
DROP TABLE sessions;

-- Rename new table
ALTER TABLE sessions_new RENAME TO sessions;

-- Recreate indexes
CREATE INDEX idx_sessions_created_at ON sessions(created_at);
CREATE INDEX idx_sessions_status ON sessions(status);

-- Recreate trigger
CREATE TRIGGER update_sessions_updated_at
AFTER UPDATE ON sessions
FOR EACH ROW
BEGIN
    UPDATE sessions SET updated_at = CURRENT_TIMESTAMP WHERE id = NEW.id;
END;
