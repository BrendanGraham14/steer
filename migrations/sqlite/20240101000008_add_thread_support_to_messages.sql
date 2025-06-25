-- Add thread support to messages table
ALTER TABLE messages ADD COLUMN thread_id BLOB NOT NULL DEFAULT X'00000000000000000000000000000000';
ALTER TABLE messages ADD COLUMN parent_message_id TEXT;

-- Create indexes for efficient thread and parent lookups
CREATE INDEX idx_messages_thread_id ON messages(thread_id);
CREATE INDEX idx_messages_parent_id ON messages(parent_message_id);

-- Add foreign key constraint for parent_message_id (self-referential)
-- Note: SQLite doesn't support adding foreign keys to existing tables,
-- so this is mainly for documentation. The constraint will be enforced
-- at the application level.