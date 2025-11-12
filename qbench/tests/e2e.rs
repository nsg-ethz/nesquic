use utils::bin::{Client, ClientArgs, Server, ServerArgs};

pub async fn establish_connection<C: Client, S: Server + Send>() {
    tokio::spawn(async move {
        let mut server = S::new(ServerArgs::test()).expect("server::new");
        let res = server.listen().await;
        assert!(res.is_ok());
    });

    let mut client = C::new(ClientArgs::test()).expect("client::new");
    let res = client.run().await;
    assert!(res.is_ok(), "{}", res.err().unwrap());
    assert!(client.stats().throughputs().mean() > 0.0);
}

#[tokio::test]
async fn quinn_uploads_data_to_quinn() {
    establish_connection::<quinn_iut::Client, quinn_iut::Server>().await;
}

#[tokio::test]
async fn quinn_uploads_data_to_quiche() {
    establish_connection::<quinn_iut::Client, quiche_iut::Server>().await;
}
