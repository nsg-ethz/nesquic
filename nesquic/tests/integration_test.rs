use anyhow::Result;
use nesquic::{run_client, run_server, Library};
use std::time::Duration;
use tracing::trace;
use utils::bin::{Client, ClientArgs, ServerArgs};

#[cfg(feature = "quinn")]
use quinn_iut::Client as HealthClient;
#[cfg(feature = "quiche")]
use quiche_iut::Client as HealthClient;
#[cfg(feature = "neqo")]
use neqo_iut::Client as HealthClient;
#[cfg(feature = "noq")]
use noq_iut::Client as HealthClient;
// #[cfg(feature = "msquic")]
// use msquic_iut::Client as HealthClient;

async fn health_check() -> Result<()> {
    let mut client = HealthClient::new(ClientArgs::test())?;
    client.connect().await?;
    Ok(())
}


#[tokio::test]
async fn library_tests() {
    #[cfg(feature = "quinn")]
    let library = Library::Quinn;
    #[cfg(feature = "quiche")]
    let library = Library::Quiche;
    #[cfg(feature = "neqo")]
    let library = Library::Neqo;
    #[cfg(feature = "noq")]
    let library = Library::Noq;
    // #[cfg(feature = "msquic")]
    // let library = Library::Msquic;

    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::FULL)
        .try_init();

    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let res = run_server(library, ServerArgs::test()).await;
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
                Duration::from_secs(5),
                run_client(library, ClientArgs::test()),
            )
            .await;
            assert!(res.is_ok(), "Test timed out");

            let res = res.unwrap();
            assert!(res.is_ok(), "{}", res.err().unwrap());
        })
        .await;
}
