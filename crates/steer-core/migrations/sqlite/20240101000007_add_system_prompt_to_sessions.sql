-- Add the system_prompt column to the sessions table to store custom system prompts.
-- It is a TEXT column that can be NULL, indicating that the default system prompt should be used.
ALTER TABLE sessions ADD COLUMN system_prompt TEXT;