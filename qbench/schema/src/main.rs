use sea_schema::postgres::discovery::SchemaDiscovery;
use sqlx::PgPool;

#[async_std::main]
async fn main() {
    let url = "postgres://postgres:postgres@localhost/qbench_dev";
    let conn = PgPool::connect(&url).await.unwrap();
    let schema_discovery = SchemaDiscovery::new(conn, "public");
    let schema = schema_discovery.discover().await.unwrap();

    println!("{:#?}", schema);
}
