-- Create session_snapshots table
CREATE TABLE IF NOT EXISTS session_snapshots (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    event_sequence_num INTEGER NOT NULL, -- Events up to this point are included
    snapshot_data TEXT NOT NULL, -- Full SessionState serialized as JSON
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    
    FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
);

-- Create index for finding latest snapshot for a session
CREATE INDEX idx_snapshots_session_id ON session_snapshots(session_id, created_at DESC);