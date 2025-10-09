use ringbuffer::{ConstGenericRingBuffer, RingBuffer};
use std::io;
use std::io::ErrorKind::WouldBlock;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd};

const UDP_BUF_SIZE: usize = 1452;
const RING_BUF_SIZE: usize = 64;
type UBuf = [u8; UDP_BUF_SIZE];

#[derive(Debug, Clone)]
pub struct Packet<const N: usize = UDP_BUF_SIZE> {
    pub data: [u8; N],
    pub len: usize,
    pub dst: SocketAddr,
}

#[derive(Debug)]
pub struct BufferedSocket {
    pub socket: UdpSocket,
    pub writable: bool,
    pub buffer: ConstGenericRingBuffer<Packet, RING_BUF_SIZE>,
}

impl BufferedSocket {
    pub fn new(ip: IpAddr, port: u16) -> io::Result<BufferedSocket> {
        let socket = UdpSocket::bind((ip, port))?;
        socket.set_nonblocking(true)?;
        let buffer = ConstGenericRingBuffer::new();
        Ok(BufferedSocket {
            socket,
            buffer,
            writable: true,
        })
    }

    /// try to send a single packet and update the writable state of the socket
    /// buffers the packet if it's not writable
    pub fn send_one(&mut self, pkt: Packet) -> io::Result<()> {
        if self.writable {
            let r = self.socket.send_to(&pkt.data[0..pkt.len], pkt.dst);
            match r {
                Ok(_bytes) => Ok(()),
                Err(ref e) if e.kind() == WouldBlock => {
                    self.writable = false;
                    self.try_enqueue(pkt);
                    Ok(())
                }
                Err(e) => Err(e),
            }
        } else {
            self.try_enqueue(pkt);
            Ok(())
        }
    }

    /// try to send all packets in the buffer (handles EAGAIN)
    pub fn try_send(&mut self) -> io::Result<()> {
        if self.writable {
            let r: io::Result<()> = {
                while let Some(pkt) = self.buffer.peek() {
                    self.socket.send_to(&pkt.data[0..pkt.len], pkt.dst)?;
                    self.buffer.dequeue();
                }
                Ok(())
            };
            match r {
                Ok(()) => Ok(()),
                Err(ref e) if e.kind() == WouldBlock => {
                    self.writable = false;
                    Ok(())
                }
                Err(e) => Err(e),
            }
        } else {
            Ok(())
        }
    }

    pub fn receive_(
        &self,
        mut cb: impl FnMut(UBuf, usize, SocketAddr) -> io::Result<()>,
    ) -> io::Result<()> {
        loop {
            let mut data = [0; UDP_BUF_SIZE];
            let (len, src) = self.socket.recv_from(&mut data)?;
            cb(data, len, src)?
        }
    }

    /// receive packets until there's no packet left and hits EAGAIN
    /// returns `Ok(())` if the error is EAGAIN and passes the error otherwise
    pub fn receive(
        &self,
        cb: impl FnMut(UBuf, usize, SocketAddr) -> io::Result<()>,
    ) -> io::Result<()> {
        let r = self.receive_(cb);
        match r {
            Ok(()) => {
                panic!("impossible: infinite loop returns")
            }
            Err(ref e) if e.kind() == WouldBlock => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn try_enqueue(&mut self, pkt: Packet) {
        if self.buffer.is_full() {
            // TODO: proper logging
            eprintln!("waring: packet dropped due to buffer overflow")
        } else {
            self.buffer.enqueue(pkt);
        }
    }
}

impl AsRawFd for BufferedSocket {
    fn as_raw_fd(&self) -> RawFd {
        self.socket.as_raw_fd()
    }
}

impl AsFd for BufferedSocket {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.socket.as_fd()
    }
}

#[cfg(test)]
mod tests {}
