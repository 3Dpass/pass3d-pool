#![feature(async_closure)]

use std::sync::Arc;
use std::thread;
use structopt::StructOpt;

use crate::rpc::{MiningContext, P3dParams};

mod rpc;
mod worker;

#[derive(StructOpt)]
struct Cli {
    /// 3d hash algorithm
    #[structopt(short, long)]
    /// Mining algorithm. Supported algorithms: grid2d, grid2d_v2, grid2d_v3
    algo: String,

    #[structopt(short, long)]
    /// Number of threads
    threads: Option<u16>,

    #[structopt(default_value = "http://127.0.0.1:9933", short, long)]
    /// Pool url
    url: String,

    #[structopt(short, long)]
    /// Pool AccountId
    pool_id: String,

    #[structopt(short, long)]
    /// Pool member AccountId
    member_id: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Cli::from_args();
    let p3d_params = P3dParams::new(args.algo.as_str());
    let ctx = MiningContext::new(p3d_params, args.url.as_str(), args.pool_id, args.member_id)?;

    let ctx = Arc::new(ctx);
    let _addr = rpc::run_server(ctx.clone()).await?;

    let ctxx = ctx.clone();
    tokio::spawn(worker::node_client(ctxx));

    for _ in 0..args.threads.unwrap_or(1) {
        let ctx = ctx.clone();
        thread::spawn(move || {
            // Process each socket concurrently.
            worker::worker(&ctx);
        });
    }

    worker::start_timer(ctx.clone());

    futures::future::pending().await
}
