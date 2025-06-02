-- Create sessions table
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    status TEXT NOT NULL CHECK (status IN ('active', 'inactive')), -- Runtime status managed by SessionManager
    metadata TEXT NOT NULL DEFAULT '{}', -- JSON stored as TEXT in SQLite

    -- Tool approval configuration
    tool_policy_type TEXT NOT NULL CHECK (tool_policy_type IN ('always_ask', 'pre_approved', 'mixed')),
    pre_approved_tools TEXT NOT NULL DEFAULT '[]', -- JSON array stored as TEXT

    -- Session configuration
    tool_config TEXT NOT NULL DEFAULT '{}' -- JSON stored as TEXT
);

-- Create indexes for common queries
CREATE INDEX idx_sessions_created_at ON sessions(created_at);
CREATE INDEX idx_sessions_status ON sessions(status);

-- Create trigger to update updated_at timestamp
CREATE TRIGGER update_sessions_updated_at
AFTER UPDATE ON sessions
FOR EACH ROW
BEGIN
    UPDATE sessions SET updated_at = CURRENT_TIMESTAMP WHERE id = NEW.id;
END;
