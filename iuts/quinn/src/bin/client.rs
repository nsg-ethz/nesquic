use clap::Parser;
use common::{
    args::ClientArgs,
    perf::{parse_blob_size, Stats},
};
use log::error;
use quinn_iut::client;

#[tokio::main]
async fn main() {
    env_logger::init();

    let args = ClientArgs::parse();
    let blob_size = parse_blob_size(&args.blob).expect("didn't recognize blob size");
    let mut stats = Stats::new(blob_size);
    let mut code = 0;

    for _ in 0..args.reps {
        if let Err(e) = client::run(&args, &mut stats).await {
            error!("Client connection error: {e}");
            code = 1;
            break;
        }
    }

    println!("{}", stats.summary());
    std::process::exit(code);
}
