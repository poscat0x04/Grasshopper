use std::io;
use std::io::ErrorKind::WouldBlock;

pub fn cvt_send_res(r: io::Result<usize>) -> io::Result<()> {
    match r {
        Ok(_sent) => Ok(()),
        Err(ref e) if e.kind() == WouldBlock => Ok(()),
        Err(e) => Err(e),
    }
}

pub fn cvt_recv_res(r: io::Result<()>) -> io::Result<()> {
    match r {
        Ok(()) => {
            panic!("impossible: return from infinite loop")
        }
        Err(ref e) if e.kind() == WouldBlock => Ok(()),
        Err(e) => Err(e),
    }
}
