use crate::bin::{Client, ClientArgs, Server, ServerArgs};

pub async fn establish_connection<C: Client, S: Server + Send>() {
    tokio::spawn(async move {
        let server = S::new(ServerArgs::test()).expect("server::new");
        let res = server.listen().await;
        assert!(res.is_ok());
    });

    let mut client = C::new(ClientArgs::test()).expect("client::new");
    let res = client.run().await;
    assert!(res.is_ok(), "{}", res.err().unwrap());
}
