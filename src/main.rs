mod consul;
mod error;

use exitfailure::ExitFailure;
use futures::stream::Stream;
use std::net::IpAddr;
use structopt::StructOpt;
use structopt_flags::LogLevel;

#[derive(Debug, StructOpt)]
#[structopt(name = "ronsul", about = "A small consul utility written in rust")]
pub struct Opt {
    #[structopt(flatten)]
    verbose: structopt_flags::QuietVerbose,
    #[structopt(short = "S", long = "consul-server")]
    consul_server: Option<IpAddr>,
}

fn main() -> std::result::Result<(), ExitFailure> {
    let opt = Opt::from_args();
    opt.verbose.set_log_level();
    let consul_ips = consul::get_consul_ips(&opt)?;
    if consul_ips.is_empty() {
        println!("no consul servers found");
        return Ok(());
    }
    let base_url = format!("http://{}:8500", consul_ips.iter().next().unwrap());
    println!("Polling {}", base_url);
    let stream = consul::get_catalog_services_updates(reqwest::Url::parse(&base_url).unwrap())
        .for_each(|(added, removed)| {
            println!("------------");
            println!("added services: {}", added.len());
            for (s, _) in added.iter() {
                println!("service name: {}", s)
            }
            println!("removed services: {}", removed.len());
            for (s, _) in removed.iter() {
                println!("service name: {}", s)
            }
            Ok(())
        });
    tokio::run(stream);

    Ok(())
}
