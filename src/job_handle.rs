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
            .delete_one(doc! { "jid": self.row.jid })
            .await
            .context("Failed to mark job as completed")?;
        Ok(())
    }

    async fn fail(mut self) -> Result<(), QueueError> {
        self.collection()
            .update_one(
                doc! { "jid": self.row.jid },
                doc! { "$set": { "started_at": None::<bson::DateTime> } },
            )
            .await
            .context("Failed to mark job as failed")?;
        Ok(())
    }

    async fn dead_queue(mut self) -> Result<(), QueueError> {
        let collection = self.collection().clone();
        let dead_collection = self.dead_queue_collection().clone();
        let client = collection.client();

        let mut session = client
            .start_session()
            .await
            .context("Failed to start session")?;
        session
            .start_transaction()
            .and_run(
                (&collection, &dead_collection, &self.row),
                |session, (collection, dead_collection, row)| {
                    Box::pin(async move {
                        let jid = row.jid.clone();
                        let retries = row.retries;
                        let job_type = row.job_type.clone();
                        let payload = row.payload.clone();
                        let scheduled_at = row.scheduled_at;
                        let enqueued_at = row.enqueued_at;

                        collection
                            .delete_one(doc! { "jid": jid.clone() })
                            .session(&mut *session)
                            .await?;

                        dead_collection
                            .insert_one(JobRow {
                                jid,
                                queue: "default".to_string(),
                                job_type,
                                payload,
                                retries,
                                scheduled_at,
                                enqueued_at,
                                priority: 0,
                                started_at: None,
                            })
                            .session(session)
                            .await?;

                        Ok(())
                    })
                },
            )
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
