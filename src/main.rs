use crate::udp::{BufferedSocket, Packet};
use ah::Context;
use anyhow as ah;
use argh::from_env;
use args::{ClientOpts, ServerOpts, SubCommand, TopLevelCommand};
use indexmap::map::Entry;
use indexmap::IndexMap;
use polling::PollMode::{Edge, Oneshot};
use polling::{Event, Events, Poller};
use rand::rngs::SmallRng;
use rand::seq::SliceRandom;
use rand::SeedableRng;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{Duration, Instant};

mod args;
mod udp;

type SrcAddr = SocketAddr;
type DsSockIdx = usize;

const HOP_INTERVAL_SECS: u64 = 120;
const CONN_TIMEOUT_SECS: u64 = 240;
const POLLER_TIMEOUT_SECS: u64 = 1;

const KEY_MAX: usize = usize::MAX - 1;

fn main() -> ah::Result<()> {
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
    range_values: Box<[u16]>,
    len: usize,
    step: usize, // step in rotation
    rng: SmallRng,
}

impl ShufflingRand {
    fn new(min: u16, max: u16) -> Self {
        let range_values: Box<[u16]> = (min..=max).collect::<Vec<u16>>().into_boxed_slice();
        ShufflingRand {
            range_values,
            len: (max - min + 1) as _,
            step: 0,
            rng: SmallRng::from_os_rng(),
        }
    }

    fn next(&mut self) -> u16 {
        if self.step == 0 {
            self.range_values.shuffle(&mut self.rng)
        }
        let r = self.range_values[self.step];
        self.step = (self.step + 1) % self.len;
        r
    }
}

fn client_main(opts: &ClientOpts) -> ah::Result<()> {
    let mut events = Events::new();
    let poller = Poller::new().context("creating poller instance")?;
    let mut conns: IndexMap<SrcAddr, Connection> = IndexMap::new();
    let mut rng = ShufflingRand::new(opts.server_pr_min, opts.server_pr_max);

    let mut us_port = rng.next();
    let mut hop_deadline: Instant = Instant::now()
        .checked_add(Duration::from_secs(HOP_INTERVAL_SECS))
        .expect("impossible: Instant overflow");

    let mut ds_sock = BufferedSocket::new(opts.listen_addr.ip(), opts.listen_addr.port())
        .context("creating downstream socket")?;
    unsafe {
        poller
            .add_with_mode(&ds_sock, Event::new(KEY_MAX, true, false), Edge)
            .context("declaring interest in ds_sock's readable events")?;
    }

    loop {
        events.clear();
        poller
            .wait(&mut events, Some(Duration::from_secs(POLLER_TIMEOUT_SECS)))
            .context("waiting for events")?;
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
            if event.key == KEY_MAX {
                if event.readable {
                    ds_sock.receive(|data, len, src| {
                        let entry = conns.entry(src);
                        let pkt = Packet {
                            data,
                            len,
                            dst: SocketAddr::new(opts.server_ip, us_port),
                        };

                        match entry {
                            // established connection
                            Entry::Occupied(mut oe) => {
                                let us_sock_idx = oe.index();
                                let conn = oe.get_mut();
                                let prev_writable = conn.socket.writable;
                                conn.socket.send_one(pkt)?;
                                if prev_writable && !conn.socket.writable {
                                    unsafe {
                                        poller.add_with_mode(
                                            &conn.socket,
                                            Event::new(us_sock_idx, false, true),
                                            Oneshot,
                                        )?;
                                    }
                                }
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
                                        Event::new(ve.index(), true, false),
                                        Edge,
                                    )?;
                                }

                                let prev_writable = sock.writable;
                                sock.send_one(pkt)?;
                                if prev_writable && !sock.writable {
                                    unsafe {
                                        poller.add_with_mode(
                                            &sock,
                                            Event::new(ve.index(), false, true),
                                            Oneshot,
                                        )?;
                                    }
                                }

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
                    })?
                }

                if event.writable {
                    ds_sock.writable = true;
                    ds_sock.try_send()?;
                    if !ds_sock.writable {
                        unsafe {
                            poller.add_with_mode(
                                &ds_sock,
                                Event::new(KEY_MAX, false, true),
                                Oneshot,
                            )?;
                        }
                    }
                }
            // us_sock
            } else if let Some((dst, conn)) = conns.get_index_mut(event.key) {
                if event.readable {
                    conn.socket.receive(|data, len, src| {
                        let pkt = Packet {
                            data,
                            len,
                            dst: *dst,
                        };

                        if src.ip() == opts.server_ip
                            && (opts.server_pr_min..=opts.server_pr_max).contains(&src.port())
                        {
                            let prev_writable = ds_sock.writable;
                            ds_sock.send_one(pkt)?;
                            if prev_writable && !ds_sock.writable {
                                unsafe {
                                    poller.add_with_mode(
                                        &ds_sock,
                                        Event::new(KEY_MAX, false, true),
                                        Oneshot,
                                    )?;
                                }
                            }
                        }
                        Ok(())
                    })?;
                }

                if event.writable {
                    conn.socket.writable = true;
                    conn.socket.try_send()?;
                    if !conn.socket.writable {
                        unsafe {
                            poller.add_with_mode(
                                &conn.socket,
                                Event::new(event.key, false, true),
                                Oneshot,
                            )?
                        }
                    }
                }
            }
            // else: spurious wakeup, ignored
        }
    }
}

fn server_main(opts: &ServerOpts) -> ah::Result<()> {
    let mut events = Events::new();
    let poller = Poller::new()?;
    // additional usize for the index of the last ds_sock that sent message to
    let mut conns: IndexMap<SocketAddr, (Connection, DsSockIdx)> = IndexMap::new();
    let mut ds_socks: Vec<BufferedSocket> =
        Vec::with_capacity((opts.pr_max - opts.pr_min + 1) as _);

    for port in opts.pr_min..=opts.pr_max {
        let s = BufferedSocket::new(opts.listen_ip, port)?;
        unsafe {
            poller.add_with_mode(
                &s,
                Event::new(KEY_MAX - ((port - opts.pr_min) as usize), true, true),
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
            if (KEY_MAX - ((opts.pr_max - opts.pr_min) as usize)..=KEY_MAX).contains(&event.key) {
                let ds_sock_idx = KEY_MAX - event.key;
                let ds_sock = &mut ds_socks[ds_sock_idx];

                if event.readable {
                    ds_sock.receive(|data, len, src| {
                        let entry = conns.entry(src);
                        let pkt = Packet {
                            data,
                            len,
                            dst: opts.us_addr,
                        };

                        match entry {
                            Entry::Occupied(mut oe) => {
                                let us_sock_idx = oe.index();
                                let (conn, last_sock_idx) = oe.get_mut();
                                *last_sock_idx = ds_sock_idx;

                                let prev_writable = conn.socket.writable;
                                conn.socket.send_one(pkt)?;
                                if prev_writable && !conn.socket.writable {
                                    unsafe {
                                        poller.add_with_mode(
                                            &conn.socket,
                                            Event::new(us_sock_idx, false, true),
                                            Oneshot,
                                        )?;
                                    }
                                }
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

                                let prev_writable = sock.writable;
                                sock.send_one(pkt)?;
                                if prev_writable && !sock.writable {
                                    unsafe {
                                        poller.add_with_mode(
                                            &sock,
                                            Event::new(ve.index(), false, true),
                                            Oneshot,
                                        )?;
                                    }
                                }

                                let conn = Connection {
                                    timeout: now
                                        .checked_add(Duration::from_secs(CONN_TIMEOUT_SECS))
                                        .expect("impossible: Instant overflow"),
                                    socket: sock,
                                };
                                ve.insert((conn, ds_sock_idx));
                            }
                        }
                        Ok(())
                    })?;
                }

                if event.writable {
                    ds_sock.writable = true;
                    ds_sock.try_send()?;
                    if !ds_sock.writable {
                        unsafe {
                            poller.add_with_mode(
                                &*ds_sock,
                                Event::new(event.key, false, true),
                                Oneshot,
                            )?;
                        }
                    }
                }
            // us_sock
            } else if let Some((dst, (conn, last_sock_idx))) = conns.get_index_mut(event.key) {
                if event.readable {
                    conn.socket.receive(|data, len, src| {
                        let pkt = Packet {
                            data,
                            len,
                            dst: *dst,
                        };

                        if src == opts.us_addr {
                            let ds_sock = &mut ds_socks[*last_sock_idx];

                            let prev_writable = ds_sock.writable;
                            ds_sock.send_one(pkt)?;
                            if prev_writable && !ds_sock.writable {
                                unsafe {
                                    poller.add_with_mode(
                                        &*ds_sock,
                                        Event::new(KEY_MAX - *last_sock_idx, false, true),
                                        Oneshot,
                                    )?;
                                }
                            }
                        }
                        Ok(())
                    })?;
                }

                if event.writable {
                    conn.socket.writable = true;
                    conn.socket.try_send()?;
                    if !conn.socket.writable {
                        unsafe {
                            poller.add_with_mode(
                                &conn.socket,
                                Event::new(event.key, false, true),
                                Oneshot,
                            )?
                        }
                    }
                }
            }
            // spurious wakeup, ignored
        }
    }
}
