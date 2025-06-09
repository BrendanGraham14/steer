-- Add the workspace_config column to the sessions table to store the session's execution environment.
-- It is a TEXT column that will store a JSON object representing the WorkspaceConfig enum.
-- We provide a default value corresponding to a local workspace for all existing and new rows
-- to ensure backward compatibility and prevent null values.
ALTER TABLE sessions ADD COLUMN workspace_config TEXT NOT NULL DEFAULT '{"type":"local"}';
