
use anyhow::Result;
use utils::{bin, bin::ServerArgs};

const TARGET: &str = "neqo::server";

pub struct Server {
    args: ServerArgs,
}

impl bin::Server for Server {
    fn new(args: ServerArgs) -> Result<Self> {
        todo!()
    }

    async fn listen(&mut self) -> Result<()> {
        todo!()
    }
}

