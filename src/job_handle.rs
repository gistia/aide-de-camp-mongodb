use aide_de_camp::core::job_handle::JobHandle;
use aide_de_camp::core::queue::QueueError;
use aide_de_camp::core::{Bytes, Xid};
use anyhow::Context;
use async_trait::async_trait;
use bson::doc;
use mongodb::{Collection, Database};
use std::str::FromStr;

use crate::types::JobRow;

#[derive(Debug)]
pub struct MongoDbJobHandle {
    row: JobRow,
    database: Database,
}

#[async_trait]
impl JobHandle for MongoDbJobHandle {
    fn id(&self) -> Xid {
        Xid::from_str(&self.row.jid).unwrap()
    }

    fn job_type(&self) -> &str {
        &self.row.job_type
    }

    fn payload(&self) -> Bytes {
        self.row.payload.bytes.clone().into()
    }

    fn retries(&self) -> u32 {
        self.row.retries as u32
    }

    async fn complete(mut self) -> Result<(), QueueError> {
        self.collection()
            .delete_one(doc! { "jid": self.row.jid }, None)
            .await
            .context("Failed to mark job as completed")?;
        Ok(())
    }

    async fn fail(mut self) -> Result<(), QueueError> {
        self.collection()
            .update_one(
                doc! { "jid": self.row.jid },
                doc! { "$set": { "started_at": None::<bson::DateTime> } },
                None,
            )
            .await
            .context("Failed to mark job as failed")?;
        Ok(())
    }

    async fn dead_queue(mut self) -> Result<(), QueueError> {
        let collection = self.collection().clone();
        let dead_collection = self.dead_queue_collection().clone();
        let client = collection.client();

        let jid = self.row.jid;
        let retries = self.row.retries;
        let job_type = self.row.job_type.clone();
        let payload = self.row.payload.clone();
        let scheduled_at = self.row.scheduled_at;
        let enqueued_at = self.row.enqueued_at;

        let mut session = client
            .start_session(None)
            .await
            .context("Failed to start session")?;
        session
            .start_transaction(None)
            .await
            .context("Failed to start transaction")?;

        collection
            .delete_one_with_session(doc! { "jid": jid.clone() }, None, &mut session)
            .await
            .context("Failed to delete job from the queue")?;

        dead_collection
            .insert_one_with_session(
                JobRow {
                    jid,
                    queue: "default".to_string(),
                    job_type,
                    payload,
                    retries,
                    scheduled_at,
                    enqueued_at,
                    priority: 0,
                    started_at: None,
                },
                None,
                &mut session,
            )
            .await
            .context("Failed to mark job as dead")?;

        session
            .commit_transaction()
            .await
            .context("Failed to commit transaction")?;

        Ok(())
    }
}

impl MongoDbJobHandle {
    pub(crate) fn new(row: JobRow, database: Database) -> Self {
        Self { row, database }
    }

    fn collection(&self) -> Collection<JobRow> {
        self.database.collection("adc_queue")
    }

    fn dead_queue_collection(&self) -> Collection<JobRow> {
        self.database.collection("adc_dead_queue")
    }
}
