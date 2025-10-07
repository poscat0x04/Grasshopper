use ringbuffer::{ConstGenericRingBuffer, RingBuffer};
use std::io;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd};

const UDP_BUF_SIZE: usize = 1452;
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
    pub buffer: ConstGenericRingBuffer<Packet, 64>,
}

impl BufferedSocket {
    pub fn new(ip: IpAddr, port: u16) -> io::Result<BufferedSocket> {
        let socket = UdpSocket::bind((ip, port))?;
        socket.set_nonblocking(true)?;
        let buffer = ConstGenericRingBuffer::new();
        Ok(BufferedSocket { socket, buffer })
    }

    pub fn send_one(&mut self, pkt: &Packet) -> io::Result<()> {
        let _bytes = self.socket.send_to(&pkt.data[0..pkt.len], pkt.dst)?;
        Ok(())
    }

    pub fn recv_one(&mut self) -> io::Result<(UBuf, usize, SocketAddr)> {
        let mut data = [0; UDP_BUF_SIZE];
        let (bytes, addr) = self.socket.recv_from(&mut data)?;
        Ok((data, bytes, addr))
    }

    pub fn try_send(&mut self) -> io::Result<usize> {
        let mut total_sent = 0;
        while let Some(pkt) = self.buffer.dequeue() {
            self.send_one(&pkt)?;
            total_sent += 1;
        }
        Ok(total_sent)
    }

    pub fn try_receive(
        &mut self,
        mut cb: impl FnMut(UBuf, usize, SocketAddr) -> io::Result<()>,
    ) -> io::Result<()> {
        loop {
            let (data, len, src) = self.recv_one()?;
            cb(data, len, src)?
        }
    }

    pub fn try_enqueue(&mut self, pkt: Packet) {
        if self.buffer.is_full() {
            // silently drops packet
            // TODO: proper handling (logging etc.)
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
