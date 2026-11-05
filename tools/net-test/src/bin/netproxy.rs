//! `netproxy --listen A --upstream B --control C [--seed N]`: the channel between the bots (or browsers) and the
//! server. Prints one line `netproxy listen=<addr> control=<addr>` when it is ready.

use std::net::SocketAddr;
use std::process::ExitCode;

use clap::Parser;

#[derive(Parser)]
#[command(
    name = "netproxy",
    about = "TCP relay with delay, jitter, bandwidth, loss model, blackhole and resets"
)]
struct Cli {
    #[arg(long, default_value = "127.0.0.1:18080")]
    listen: SocketAddr,
    #[arg(long, default_value = "127.0.0.1:8080")]
    upstream: SocketAddr,
    #[arg(long, default_value = "127.0.0.1:18081")]
    control: SocketAddr,
    #[arg(long, default_value_t = 1)]
    seed: u64,
}

// One thread: the relay is light, and the hand-offs between threads cost more than they give (they show in the RTT).
#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match net_test::proxy::start(cli.listen, cli.upstream, cli.control, cli.seed).await {
        Ok(p) => {
            println!("netproxy listen={} control={}", p.listen, p.control);
            let _ = tokio::signal::ctrl_c().await;
            p.stop();
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("netproxy: {e}");
            ExitCode::FAILURE
        }
    }
}
