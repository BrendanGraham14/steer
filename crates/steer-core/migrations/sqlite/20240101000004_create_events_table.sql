-- Create events table
CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY AUTOINCREMENT, -- Auto-incrementing for SQLite
    session_id TEXT NOT NULL,
    sequence_num INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    event_data TEXT NOT NULL, -- JSON stored as TEXT
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    
    FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE,
    UNIQUE(session_id, sequence_num)
);

-- Create indexes for efficient event retrieval and replay
CREATE INDEX idx_events_session_id ON events(session_id, sequence_num);
CREATE INDEX idx_events_created_at ON events(session_id, created_at);