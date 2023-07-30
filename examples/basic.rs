use aide_de_camp::prelude::{
    CancellationToken, Duration, JobProcessor, JobRunner, Queue, RunnerOptions, RunnerRouter, Xid,
};
use aide_de_camp_mongodb::MongoDbQueue;
use async_trait::async_trait;

struct MyJob;

#[async_trait]
impl JobProcessor for MyJob {
    type Payload = Vec<u32>;
    type Error = anyhow::Error;

    async fn handle(
        &self,
        _jid: Xid,
        payload: Self::Payload,
        _cancellation_token: CancellationToken,
    ) -> Result<(), Self::Error> {
        println!("payload: {:?}", payload);
        Ok(())
    }

    fn name() -> &'static str {
        "my_job"
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let queue = MongoDbQueue::new("mongodb://localhost:27017/queues", None).await?;

    // Add job the queue to run next
    let _jid = queue.schedule::<MyJob>(vec![1, 2, 3], 0).await?;

    // First create a job processor and router
    let router = {
        let mut r = RunnerRouter::default();
        r.add_job_handler(MyJob);
        r
    };
    // Setup runner to at most 10 jobs concurrently
    let mut runner = JobRunner::new(queue, router, 10, RunnerOptions::default());
    // Poll the queue every second, this will block unless something went really wrong.
    // The future supplied as the second parameter will tell the server to shut down when it completes.
    runner
        .run_with_shutdown(Duration::seconds(1), async move {
            // To avoid blocking this doctest, run for 10 milliseconds, then initiate shutdown.
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            // In a real application, you may want to wait for a CTRL+C event or something similar.
            // You could do this with tokio using the signal module: tokio::signal::ctrl_c().await.expect("failed to install CTRL+C signal handler");
        })
        .await?;
    Ok(())
}
