-- Add columns for typed tool results
ALTER TABLE tool_calls ADD COLUMN kind TEXT;
ALTER TABLE tool_calls ADD COLUMN payload_json TEXT;
ALTER TABLE tool_calls ADD COLUMN error_json TEXT;

-- Update existing rows to use the new schema
UPDATE tool_calls 
SET 
    kind = CASE 
        WHEN status = 'failed' THEN 'error'
        WHEN tool_name = 'grep' OR tool_name = 'astgrep' THEN 'search'
        WHEN tool_name = 'ls' THEN 'file_list'
        WHEN tool_name = 'view' OR tool_name = 'read_file' THEN 'file_content'
        WHEN tool_name = 'edit' OR tool_name = 'edit_file' OR tool_name = 'multi_edit_file' THEN 'edit'
        WHEN tool_name = 'bash' THEN 'bash'
        WHEN tool_name = 'glob' THEN 'glob'
        WHEN tool_name = 'TodoRead' THEN 'todo_read'
        WHEN tool_name = 'TodoWrite' THEN 'todo_write'
        WHEN tool_name = 'web_fetch' OR tool_name = 'fetch' THEN 'fetch'
        WHEN tool_name = 'dispatch_agent' THEN 'agent'
        ELSE 'external'
    END,
    payload_json = CASE 
        WHEN status = 'completed' AND result IS NOT NULL THEN json_object(
            'tool_name', tool_name,
            'payload', result
        )
        ELSE NULL
    END,
    error_json = CASE
        WHEN status = 'failed' AND error IS NOT NULL THEN json_object(
            'tool_name', tool_name,
            'message', error
        )
        ELSE NULL
    END
WHERE kind IS NULL;

-- Create indexes for performance
CREATE INDEX IF NOT EXISTS idx_tool_calls_kind ON tool_calls(kind);