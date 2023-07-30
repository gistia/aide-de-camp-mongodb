use aide_de_camp::core::{
    job_processor::JobProcessor,
    queue::{Queue, QueueError},
    {bincode::Encode, new_xid, DateTime, Xid},
};

use anyhow::Context;
use async_trait::async_trait;
use bincode::Decode;
use bson::{doc, Binary};
use chrono::Utc;
use mongodb::{
    options::{ClientOptions, ConnectionString, FindOneOptions, Tls, TlsOptions, UpdateOptions},
    Client, Collection, Database,
};
use tracing::instrument;

use crate::{job_handle::MongoDbJobHandle, types::JobRow};

/// An implementation of the Queue backed by MongoDB
#[derive(Clone)]
pub struct MongoDbQueue {
    database: Database,
    bincode_config: bincode::config::Configuration,
}

impl MongoDbQueue {
    pub async fn new(uri: &str, cert_file: Option<String>) -> Result<Self, mongodb::error::Error> {
        let client = Self::new_client(uri, cert_file).await?;
        let database = client.default_database().unwrap_or(client.database("adc"));

        Ok(Self {
            database,
            bincode_config: bincode::config::standard(),
        })
    }

    async fn new_client(
        uri: &str,
        cert_path: Option<String>,
    ) -> Result<Client, mongodb::error::Error> {
        match cert_path {
            Some(cert_path) => {
                let conn_str = ConnectionString::parse(uri)?;
                let mut options = ClientOptions::parse_connection_string(conn_str).await?;
                let mut tls_options = TlsOptions::default();
                tls_options.ca_file_path = Some(cert_path.clone().into());
                tls_options.allow_invalid_hostnames = Some(true);
                options.tls = Some(Tls::Enabled(tls_options));
                let client = Client::with_options(options)?;
                Ok(client)
            }
            None => {
                let client = mongodb::Client::with_uri_str(uri).await?;
                Ok(client)
            }
        }
    }

    #[cfg(test)]
    pub async fn delete_database(&self) -> Result<(), mongodb::error::Error> {
        self.database.drop(None).await
    }
}

#[async_trait]
impl Queue for MongoDbQueue {
    type JobHandle = MongoDbJobHandle;

    #[instrument(skip_all, err, ret, fields(job_type = J::name(), payload_size))]
    async fn schedule_at<J>(
        &self,
        payload: J::Payload,
        scheduled_at: DateTime,
        priority: i8,
    ) -> Result<Xid, QueueError>
    where
        J: JobProcessor + 'static,
        J::Payload: Encode,
    {
        let payload = bincode::encode_to_vec(&payload, self.bincode_config)?;
        let jid = new_xid();
        let job_type = J::name();

        tracing::Span::current().record("payload_size", payload.len());

        self.database
            .collection::<JobRow>("adc_queue")
            .insert_one(
                JobRow {
                    jid: format!("{}", jid),
                    queue: "default".to_string(),
                    job_type: job_type.to_string(),
                    payload: Binary {
                        subtype: mongodb::bson::spec::BinarySubtype::Generic,
                        bytes: payload.clone(),
                    },
                    retries: 0,
                    scheduled_at: bson::DateTime::from_millis(scheduled_at.timestamp_millis()),
                    enqueued_at: bson::DateTime::from_millis(Utc::now().timestamp_millis()),
                    priority: priority as i64,
                    started_at: None,
                },
                None,
            )
            .await
            .context("Failed to add job to the queue")?;

        Ok(jid)
    }

    #[instrument(skip_all, err)]
    async fn poll_next_with_instant(
        &self,
        job_types: &[&str],
        now: DateTime,
    ) -> Result<Option<MongoDbJobHandle>, QueueError> {
        let job_types_doc = doc! {
            "$in": job_types
        };

        let filter_doc = doc! {
            "started_at": None::<bson::DateTime>,
            "queue": "default",
            "scheduled_at": {
                "$lte": bson::DateTime::from_millis(now.timestamp_millis())
            },
            "job_type": job_types_doc
        };

        let sort_doc = doc! {
            "priority": -1
        };

        let find_options = FindOneOptions::builder().sort(sort_doc).build();
        let row = self
            .collection()
            .find_one(filter_doc, find_options)
            .await
            .context("Failed to check out a job from the queue")?;

        if let Some(row) = row {
            let update_doc = doc! {
                "$set": { "started_at": bson::DateTime::from_millis(Utc::now().timestamp_millis()) },
                "$inc": { "retries": 1 }
            };
            let update_options = UpdateOptions::builder().build();

            self.collection()
                .update_one(doc! { "jid": &row.jid }, update_doc, update_options)
                .await
                .context("Failed to update job")?;

            Ok(Some(MongoDbJobHandle::new(row, self.database.clone())))
        } else {
            Ok(None)
        }
    }

    #[instrument(skip_all, err)]
    async fn cancel_job(&self, job_id: Xid) -> Result<(), QueueError> {
        let jid: String = format!("{}", job_id);
        let result = self
            .collection()
            .delete_one(
                doc! { "started_at": None::<bson::DateTime>, "jid": jid },
                None,
            )
            .await
            .context("Failed to remove job from the queue")?;

        if result.deleted_count == 0 {
            Err(QueueError::JobNotFound(job_id))
        } else {
            Ok(())
        }
    }

    #[allow(clippy::or_fun_call)]
    #[instrument(skip_all, err)]
    async fn unschedule_job<J>(&self, job_id: Xid) -> Result<J::Payload, QueueError>
    where
        J: JobProcessor + 'static,
        J::Payload: Decode,
    {
        let job_type = J::name();
        let jid: String = format!("{}", job_id);

        let filter_doc = doc! {
            "started_at": None::<bson::DateTime>,
            "jid": jid,
            "job_type": job_type
        };

        let row = self
            .collection()
            .find_one(filter_doc.clone(), None)
            .await
            .context("Failed to find job in the queue")?;

        match row {
            Some(row) => {
                let _ = self
                    .collection()
                    .delete_one(filter_doc, None)
                    .await
                    .context("Failed to remove job from the queue")?;

                let payload: Vec<u8> = row.payload.bytes;
                let (decoded, _) = bincode::decode_from_slice(&payload, self.bincode_config)?;
                Ok(decoded)
            }
            None => Err(QueueError::JobNotFound(job_id)),
        }
    }
}

impl MongoDbQueue {
    fn collection(&self) -> Collection<JobRow> {
        self.database.collection("adc_queue")
    }
}
