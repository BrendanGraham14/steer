-- Create tool_calls table
CREATE TABLE IF NOT EXISTS tool_calls (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    message_id TEXT,
    tool_name TEXT NOT NULL,
    parameters TEXT NOT NULL, -- JSON stored as TEXT
    status TEXT NOT NULL CHECK (status IN ('pending', 'approved', 'denied', 'executing', 'completed', 'failed')),
    result TEXT,
    error TEXT,
    started_at TIMESTAMP,
    completed_at TIMESTAMP,
    
    FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE,
    FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE
);

-- Create indexes for efficient querying
CREATE INDEX idx_tool_calls_session_id ON tool_calls(session_id);
CREATE INDEX idx_tool_calls_status ON tool_calls(status);