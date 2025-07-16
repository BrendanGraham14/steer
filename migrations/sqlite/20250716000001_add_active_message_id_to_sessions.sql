-- Add active_message_id column to sessions table
-- This tracks the currently active message (head of selected branch) in the conversation
ALTER TABLE sessions ADD COLUMN active_message_id TEXT;

-- Note: Column is nullable for backward compatibility
-- When NULL, the system uses the last message as the active one