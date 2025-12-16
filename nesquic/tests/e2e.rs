use anyhow::Result;
use nesquic::{run_client, run_server, Library};
use quinn_iut::Client as QuinnClient;
use std::time::Duration;
use tracing::trace;
use utils::bin::{Client, ClientArgs, ServerArgs};

async fn health_check() -> Result<()> {
    let mut client = QuinnClient::new(ClientArgs::test())?;
    client.connect().await?;
    Ok(())
}

#[test_case::test_matrix(
    [
        Library::Quinn,
        // Library::Quiche,
        Library::Msquic,
    ],
    [
        Library::Quinn,
        Library::Quiche,
        // Library::Msquic,
    ]
)]
#[tokio::test]
async fn library_tests(client: Library, server: Library) {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::FULL)
        .try_init();

    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let res = run_server(server, ServerArgs::test()).await;
        assert!(res.is_ok(), "{}", res.err().unwrap());
    });

    local
        .run_until(async move {
            for _ in 0..30 {
                let timeout = Duration::from_millis(300);
                let healthy = tokio::time::timeout(timeout, health_check()).await;
                if let Ok(Ok(_)) = healthy {
                    trace!("Server is healthy");
                    break;
                }
            }

            let res = tokio::time::timeout(
                Duration::from_secs(1),
                run_client(client, ClientArgs::test()),
            )
            .await;
            assert!(res.is_ok(), "Test timed out");

            let res = res.unwrap();
            assert!(res.is_ok(), "{}", res.err().unwrap());
        })
        .await;
}
