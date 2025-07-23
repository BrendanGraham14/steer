-- Drop the index first
DROP INDEX IF EXISTS idx_messages_thread_id;

-- Remove thread_id column from messages table
ALTER TABLE messages DROP COLUMN thread_id;