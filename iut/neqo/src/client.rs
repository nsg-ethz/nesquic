use anyhow::Result;
use utils::{bin, bin::ClientArgs};

const TARGET: &str = "neqo::client";

pub struct Client {
    args: ClientArgs
}

impl bin::Client for Client {
    fn new(args: ClientArgs) -> Result<Self> {
        todo!()
    }

    async fn connect(&mut self) -> Result<()> {
        todo!()
    }

    async fn run(&mut self) -> Result<()> {
        todo!()
    }
}
