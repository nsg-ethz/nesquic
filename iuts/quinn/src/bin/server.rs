use clap::Parser;
use common::args::ServerArgs;
use log::error;
use quinn_iut::server;

#[tokio::main]
async fn main() {
    env_logger::init();

    let args = ServerArgs::parse();
    let code = {
        if let Err(e) = server::run(args).await {
            error!("Server connection error: {e}");
            1
        } else {
            0
        }
    };
    std::process::exit(code);
}
