use crate::{run_client, run_server};
use anyhow::Result;
use std::time::Duration;
use tracing::trace;
use utils::bin::{Client, ClientArgs, Server, ServerArgs};

async fn health_check<C: Client>() -> Result<()> {
    let mut client = C::new(ClientArgs::test())?;
    client.connect().await?;
    Ok(())
}

pub async fn connectivity<C: Client, S: Server>() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::FULL)
        .try_init();

    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let res = run_server::<S>(ServerArgs::test()).await;
        assert!(res.is_ok(), "{}", res.err().unwrap());
    });

    local
        .run_until(async move {
            for _ in 0..30 {
                let timeout = Duration::from_millis(300);
                let healthy = tokio::time::timeout(timeout, health_check::<C>()).await;
                if let Ok(Ok(_)) = healthy {
                    trace!("Server is healthy");
                    break;
                }
            }

            let res =
                tokio::time::timeout(Duration::from_secs(5), run_client::<C>(ClientArgs::test()))
                    .await;
            assert!(res.is_ok(), "Test timed out");

            let res = res.unwrap();
            assert!(res.is_ok(), "{}", res.err().unwrap());
        })
        .await;
}
