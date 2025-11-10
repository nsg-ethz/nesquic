use common::{args::ClientArgs, args::ServerArgs, perf::Stats};
use quinn_iut::{client, server};

#[tokio::test]
async fn establish_connection() {
    tokio::spawn(async move {
        let res = server::run(ServerArgs::test()).await;
        assert!(res.is_ok());
        println!("Server started");
    });

    let client_args = ClientArgs::test();
    let mut stats = Stats::new(123);
    let res = client::run(&client_args, &mut stats).await;
    assert!(res.is_ok(), "{}", res.err().unwrap());
}
