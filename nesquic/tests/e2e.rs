use utils::bin::{Client, ClientArgs, Server, ServerArgs};

async fn run<C: Client, S: Server + Send>() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::FULL)
        .try_init();

    tokio::spawn(async {
        let mut server = S::new(ServerArgs::test()).expect("server::new");
        let res = server.listen().await;
        assert!(res.is_ok());
    });

    // TODO: better health check
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let mut client = C::new(ClientArgs::test()).expect("client::new");
    let res = client.run().await;
    assert!(res.is_ok(), "{}", res.err().unwrap());
    assert!(client.stats().throughputs().mean() > 0.0);
}

#[tokio::test]
async fn run_quinn_quinn() {
    run::<quinn_iut::Client, quinn_iut::Server>().await;
}

// #[tokio::test]
// async fn run_quiche_quiche() {
//     run::<quiche_iut::Client, quiche_iut::Server>().await;
// }

// #[tokio::test]
// async fn run_quinn_quiche() {
//     run::<quinn_iut::Client, quiche_iut::Server>().await;
// }

#[tokio::test]
async fn run_quiche_quinn() {
    run::<quiche_iut::Client, quinn_iut::Server>().await;
}

// #[tokio::test]
// async fn run_msquic_msquic() {
//     run::<msquic_iut::Client, msquic_iut::Server>().await;
// }

#[tokio::test]
async fn run_msquic_quinn() {
    run::<msquic_iut::Client, quinn_iut::Server>().await;
}
