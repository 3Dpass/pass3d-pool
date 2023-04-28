#![feature(async_closure)]

use std::sync::Arc;
use std::thread;

use bip39::{Language, Mnemonic};
use structopt::StructOpt;
use substrate_bip39::mini_secret_from_entropy;

use crate::rpc::{MiningContext, P3dParams};

mod rpc;
mod worker;

#[derive(Debug, StructOpt)]
enum SubCommand {
    #[structopt(name = "run", about = "Use run to start pool client")]
    Run(RunOptions),
    #[structopt(name = "inspect", about = "Use inspect to convert seed to key")]
    Inspect(InspectOptions),
}

#[derive(Debug, StructOpt)]
struct RunOptions {
    /// 3d hash algorithm
    #[structopt(default_value = "grid2d_v2", short, long)]
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

    #[structopt(short, long)]
    /// Member key to sign requests
    key: String,
}

#[derive(Debug, StructOpt)]
struct InspectOptions {
    #[structopt(short, long)]
    /// Seed phrase
    seed: String,
}

#[derive(StructOpt)]
struct Cli {
    #[structopt(subcommand)]
    cmd: SubCommand,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Cli::from_args();

    match args.cmd {
        SubCommand::Inspect(opt) => {
            let mnemonic = Mnemonic::from_phrase(&opt.seed, Language::English);
            match mnemonic {
                Ok(mnemonic) =>
                    match mini_secret_from_entropy(mnemonic.entropy(), "") {
                        Ok(mini_key) => println!("{}", hex::encode(mini_key.to_bytes())),
                        Err(e) => println!("{:?}", e),
                    },
                Err(e) => println!("{:?}", e),
            };
            Ok(())
        }
        SubCommand::Run(opt) => {
            let p3d_params = P3dParams::new(opt.algo.as_str());
            let ctx = MiningContext::new(p3d_params, opt.url.as_str(), opt.pool_id, opt.member_id, opt.key)?;
            let ctx = Arc::new(ctx);
            let _addr = rpc::run_server(ctx.clone()).await?;
            let ctx_clone = ctx.clone();
            tokio::spawn(worker::node_client(ctx_clone));

            for _ in 0..opt.threads.unwrap_or(1) {
                let ctx = ctx.clone();
                thread::spawn(move || {
                    // Process each socket concurrently.
                    worker::worker(&ctx);
                });
            }
            worker::start_timer(ctx.clone());
            futures::future::pending().await
        }
    }
}
