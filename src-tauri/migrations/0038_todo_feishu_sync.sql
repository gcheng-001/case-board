-- Case todo Feishu sync ids.
ALTER TABLE case_todos ADD COLUMN feishu_record_id TEXT;
ALTER TABLE case_todos ADD COLUMN feishu_calendar_event_id TEXT;

CREATE INDEX IF NOT EXISTS idx_case_todos_feishu_record ON case_todos(feishu_record_id);
CREATE INDEX IF NOT EXISTS idx_case_todos_feishu_calendar ON case_todos(feishu_calendar_event_id);
