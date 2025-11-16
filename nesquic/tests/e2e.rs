use utils::bin::{Client, ClientArgs, Server, ServerArgs};

async fn run<C: Client, S: Server + Send>() {
    env_logger::init();

    tokio::spawn(async {
        let mut server = S::new(ServerArgs::test()).expect("server::new");
        let res = server.listen().await;
        assert!(res.is_ok());
    });

    let mut client = C::new(ClientArgs::test()).expect("client::new");
    let res = client.run().await;
    println!("Client run result: {:?}", res);
    assert!(res.is_ok(), "{}", res.err().unwrap());
    assert!(client.stats().throughputs().mean() > 0.0);
}

#[tokio::test]
async fn run_quinn() {
    run::<quinn_iut::Client, quinn_iut::Server>().await;
}

// #[tokio::test]
// async fn run_quiche() {
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

#[tokio::test]
async fn run_msquic_quinn() {
    run::<msquic_iut::Client, quinn_iut::Server>().await;
}
