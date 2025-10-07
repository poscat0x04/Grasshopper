use crate::udp::{BufferedSocket, Packet};
use argh::{from_env, FromArgs};
use indexmap::map::Entry;
use indexmap::IndexMap;
use polling::PollMode::Edge;
use polling::{Event, Events, Poller};
use rand::rngs::SmallRng;
use rand::seq::SliceRandom;
use rand::SeedableRng;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{Duration, Instant};
use args::{ClientOpts, ServerOpts, SubCommand, TopLevelCommand};

mod udp;
mod utils;
mod args;

const HOP_INTERVAL_SECS: u64 = 120;
const CONN_TIMEOUT_SECS: u64 = 240;
const POLLER_TIMEOUT_SECS: u64 = 30;

fn main() -> io::Result<()> {
    let cmd: TopLevelCommand = from_env();
    match cmd.inner {
        SubCommand::Client(opts) => client_main(&opts),
        SubCommand::Server(opts) => server_main(&opts),
    }
}

#[derive(Debug)]
struct Connection {
    timeout: Instant,
    socket: BufferedSocket,
}

struct ShufflingRand {
    l: Box<[u16]>,
    len: usize,
    step: usize, // step in rotation
    rng: SmallRng,
}

impl ShufflingRand {
    fn new(min: u16, max: u16) -> Self {
        let l: Box<[u16]> = (min..=max).collect::<Vec<u16>>().into_boxed_slice();
        ShufflingRand {
            l,
            len: (max - min + 1) as _,
            step: 0,
            rng: SmallRng::from_os_rng(),
        }
    }

    fn next(&mut self) -> u16 {
        if self.step == 0 {
            self.l.shuffle(&mut self.rng)
        }
        let r = self.l[self.step];
        self.step = (self.step + 1) % self.len;
        r
    }
}

fn client_main(opts: &ClientOpts) -> io::Result<()> {
    let mut events = Events::new();
    let poller = Poller::new()?;
    let mut conns: IndexMap<SocketAddr, Connection> = IndexMap::new();
    let mut rng = ShufflingRand::new(opts.server_pr_min, opts.server_pr_max);
    let mut us_port = rng.next();
    let mut hop_deadline: Instant = Instant::now()
        .checked_add(Duration::from_secs(HOP_INTERVAL_SECS))
        .expect("impossible: Instant overflow");

    let mut ds_sock = BufferedSocket::new(opts.listen_addr.ip(), opts.listen_addr.port())?;
    unsafe {
        poller.add_with_mode(&ds_sock, Event::new(usize::MAX - 1, true, true), Edge)?;
    }

    loop {
        events.clear();
        poller.wait(&mut events, Some(Duration::from_secs(POLLER_TIMEOUT_SECS)))?;
        let now = Instant::now();

        // first handle timeouts
        conns.retain(|_, conn| {
            if conn.timeout > now {
                true
            } else {
                poller
                    .delete(&conn.socket)
                    .expect("failed to unregister event handle");
                false
            }
        });

        // then update upstream port we are targeting
        if now > hop_deadline {
            us_port = rng.next();
            hop_deadline = hop_deadline
                .checked_add(Duration::from_secs(HOP_INTERVAL_SECS))
                .expect("impossible: Instant overflow");
        }

        // then events
        for event in events.iter() {
            // ds_sock
            if event.key == usize::MAX - 1 {
                if event.readable {
                    let r = ds_sock.try_receive(|data, len, src| {
                        let entry = conns.entry(src);
                        let pkt = Packet {
                            data,
                            len,
                            dst: SocketAddr::new(opts.server_ip, us_port),
                        };
                        match entry {
                            // established connection
                            Entry::Occupied(mut oe) => {
                                let conn = oe.get_mut();
                                conn.socket.try_enqueue(pkt);
                            }
                            // new connection
                            Entry::Vacant(ve) => {
                                let mut sock = match opts.server_ip {
                                    IpAddr::V4(_) => {
                                        BufferedSocket::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)
                                    }
                                    IpAddr::V6(_) => {
                                        BufferedSocket::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0)
                                    }
                                }?;
                                unsafe {
                                    poller.add_with_mode(
                                        &sock,
                                        Event::new(ve.index(), true, true),
                                        Edge,
                                    )?;
                                }
                                sock.try_enqueue(pkt);
                                let conn = Connection {
                                    timeout: now
                                        .checked_add(Duration::from_secs(CONN_TIMEOUT_SECS))
                                        .expect("impossible: Instant overflow"),
                                    socket: sock,
                                };
                                ve.insert(conn);
                            }
                        }
                        Ok(())
                    });
                    utils::cvt_recv_res(r)?;
                }

                if event.writable {
                    let r = ds_sock.try_send();
                    utils::cvt_send_res(r)?;
                }
            // us_sock
            } else if let Some((dst, conn)) = conns.get_index_mut(event.key) {
                if event.readable {
                    let r = conn.socket.try_receive(|data, len, src| {
                        if src.ip() == opts.server_ip
                            && (opts.server_pr_min..=opts.server_pr_max).contains(&src.port())
                        {
                            ds_sock.try_enqueue(Packet {
                                data,
                                len,
                                dst: *dst,
                            })
                        }
                        Ok(())
                    });
                    utils::cvt_recv_res(r)?;
                }

                if event.writable {
                    let r = conn.socket.try_send();
                    utils::cvt_send_res(r)?;
                }
            }
            // else: spurious wakeup, ignored
        }
    }
}

fn server_main(opts: &ServerOpts) -> io::Result<()> {
    let mut events = Events::new();
    let poller = Poller::new()?;
    // additional usize for the index of the last ds_sock that sent message to
    let mut conns: IndexMap<SocketAddr, (Connection, usize)> = IndexMap::new();
    let mut ds_socks: Vec<BufferedSocket> =
        Vec::with_capacity((opts.pr_max - opts.pr_min + 1) as _);

    for port in opts.pr_min..=opts.pr_max {
        let s = BufferedSocket::new(opts.listen_ip, port)?;
        unsafe {
            poller.add_with_mode(
                &s,
                Event::new(usize::MAX - 1 - ((port - opts.pr_min) as usize), true, true),
                Edge,
            )?;
        }
        ds_socks.push(s);
    }

    loop {
        events.clear();
        poller.wait(&mut events, Some(Duration::from_secs(POLLER_TIMEOUT_SECS)))?;
        let now = Instant::now();

        // handle timeouts
        conns.retain(|_, (conn, _)| {
            if conn.timeout > now {
                true
            } else {
                poller
                    .delete(&conn.socket)
                    .expect("failed to unregister event handle");
                false
            }
        });

        // then events
        for event in events.iter() {
            // ds_sock
            if (usize::MAX - 1 - ((opts.pr_max - opts.pr_min) as usize)..=usize::MAX - 1)
                .contains(&event.key)
            {
                let idx = usize::MAX - 1 - event.key;
                let ds_sock = &mut ds_socks[idx];

                if event.readable {
                    let r = ds_sock.try_receive(|data, len, src| {
                        let entry = conns.entry(src);
                        let pkt = Packet {
                            data,
                            len,
                            dst: opts.us_addr,
                        };
                        match entry {
                            Entry::Occupied(mut oe) => {
                                let (conn, last_sock_idx) = oe.get_mut();
                                *last_sock_idx = idx;
                                conn.socket.try_enqueue(pkt);
                            }
                            Entry::Vacant(ve) => {
                                let mut sock = match opts.us_addr {
                                    SocketAddr::V4(_) => {
                                        BufferedSocket::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)
                                    }
                                    SocketAddr::V6(_) => {
                                        BufferedSocket::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0)
                                    }
                                }?;
                                unsafe {
                                    poller.add_with_mode(
                                        &sock,
                                        Event::new(ve.index(), true, true),
                                        Edge,
                                    )?;
                                }
                                sock.try_enqueue(pkt);
                                let conn = Connection {
                                    timeout: now
                                        .checked_add(Duration::from_secs(CONN_TIMEOUT_SECS))
                                        .expect("impossible: Instant overflow"),
                                    socket: sock,
                                };
                                ve.insert((conn, idx));
                            }
                        }
                        Ok(())
                    });
                    utils::cvt_recv_res(r)?;
                }

                if event.writable {
                    let r = ds_sock.try_send();
                    utils::cvt_send_res(r)?;
                }
            // us_sock
            } else if let Some((dst, (conn, last_sock_idx))) = conns.get_index_mut(event.key) {
                if event.readable {
                    let r = conn.socket.try_receive(|data, len, src| {
                        if src == opts.us_addr {
                            let ds_sock = &mut ds_socks[*last_sock_idx];
                            ds_sock.try_enqueue(Packet {
                                data,
                                len,
                                dst: *dst,
                            })
                        }
                        Ok(())
                    });
                    utils::cvt_recv_res(r)?;
                }

                if event.writable {
                    let r = conn.socket.try_send();
                    utils::cvt_send_res(r)?;
                }
            }
            // spurious wakeup, ignored
        }
    }
}
