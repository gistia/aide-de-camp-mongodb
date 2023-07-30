use bson::{Binary, DateTime};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct JobRow {
    pub jid: String,
    pub queue: String,
    pub job_type: String,
    pub payload: Binary,
    pub retries: i64,
    pub priority: i64,
    pub scheduled_at: DateTime,
    pub enqueued_at: DateTime,
    pub started_at: Option<DateTime>,
}
