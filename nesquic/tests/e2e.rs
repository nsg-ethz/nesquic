use anyhow::Result;
use std::time::Duration;
use tracing::trace;
use utils::bin::{Client, ClientArgs, Server, ServerArgs};

async fn health_check<C: Client>() -> Result<()> {
    let mut client = C::new(ClientArgs::test())?;
    client.connect().await?;
    Ok(())
}

async fn run<C: Client, S: Server + Send>() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::FULL)
        .try_init();

    let local = tokio::task::LocalSet::new();
    local.spawn_local(async {
        let mut server = S::new(ServerArgs::test()).expect("server::new");
        let res = server.listen().await;
        assert!(res.is_ok(), "{}", res.err().unwrap());
    });

    local
        .run_until(async {
            for _ in 0..30 {
                let timeout = Duration::from_millis(300);
                let healthy = tokio::time::timeout(timeout, health_check::<C>()).await;
                if let Ok(Ok(_)) = healthy {
                    trace!("Server is healthy");
                    break;
                }
            }

            let mut client = C::new(ClientArgs::test()).expect("client::new");
            client.connect().await.expect("client::connect");
            let res = tokio::time::timeout(Duration::from_secs(1), client.run()).await;
            assert!(res.is_ok(), "Test timed out");

            let res = res.unwrap();
            assert!(res.is_ok(), "{}", res.err().unwrap());
        })
        .await;
}

// quinn client

#[tokio::test]
async fn run_quinn_quinn() {
    run::<quinn_iut::Client, quinn_iut::Server>().await;
}

#[tokio::test]
async fn run_quinn_quiche() {
    run::<quinn_iut::Client, quiche_iut::Server>().await;
}

// #[tokio::test]
// async fn run_quinn_msquic() {
//     run::<quinn_iut::Client, msquic_iut::Server>().await;
// }

// quiche client

#[tokio::test]
async fn run_quiche_quiche() {
    run::<quiche_iut::Client, quiche_iut::Server>().await;
}

#[tokio::test]
async fn run_quiche_quinn() {
    run::<quiche_iut::Client, quinn_iut::Server>().await;
}

// #[tokio::test]
// async fn run_quiche_msquic() {
//     run::<quiche_iut::Client, msquic_iut::Server>().await;
// }

// msquic client

// #[tokio::test]
// async fn run_msquic_msquic() {
//     run::<msquic_iut::Client, msquic_iut::Server>().await;
// }

#[tokio::test]
async fn run_msquic_quinn() {
    run::<msquic_iut::Client, quinn_iut::Server>().await;
}

#[tokio::test]
async fn run_msquic_quiche() {
    run::<msquic_iut::Client, quiche_iut::Server>().await;
}
