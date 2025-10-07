use std::net::{IpAddr, SocketAddr};
use argh::FromArgs;

#[derive(FromArgs, Debug)]
#[argh(description = "udp port-hopping middleware for combating ISP UDP throttling")]
pub struct TopLevelCommand {
    #[argh(subcommand)]
    pub inner: SubCommand,
}

#[derive(FromArgs, Debug)]
#[argh(subcommand)]
pub enum SubCommand {
    Client(ClientOpts),
    Server(ServerOpts),
}

#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "server")]
#[argh(description = "starts server side middleware")]
pub struct ServerOpts {
    #[argh(option, short = 'l', long = "listen")]
    #[argh(description = "the ip address for ports to bind to")]
    pub listen_ip: IpAddr,
    #[argh(option, short = 'm', long = "min")]
    #[argh(description = "lower bound (inclusive) of the port range")]
    pub pr_min: u16, // port range min
    #[argh(option, short = 'M', long = "max")]
    #[argh(description = "upper bound (inclusive) of the port range")]
    pub pr_max: u16, // port range max
    #[argh(description = "the upstream server to forward to")]
    #[argh(option, short = 'u', long = "upstream")]
    pub us_addr: SocketAddr,
}

#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "client")]
#[argh(description = "starts client side middleware")]
pub struct ClientOpts {
    #[argh(option, short = 'l', long = "listen")]
    #[argh(description = "the address for accepting client traffic")]
    pub listen_addr: SocketAddr, // downstream socket addr
    #[argh(option, short = 's', long = "server")]
    #[argh(description = "ip of the server side middleware")]
    pub server_ip: IpAddr,
    #[argh(option, short = 'm', long = "min")]
    #[argh(description = "lower bound (inclusive) of the port range")]
    pub server_pr_min: u16,
    #[argh(option, short = 'M', long = "max")]
    #[argh(description = "upper bound (inclusive) of the port range")]
    pub server_pr_max: u16,
}