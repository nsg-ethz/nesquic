use quinn_iut::{Client, Server};
use utils::test;

#[tokio::test]
async fn establish_connection() {
    test::establish_connection::<Client, Server>().await;
}
