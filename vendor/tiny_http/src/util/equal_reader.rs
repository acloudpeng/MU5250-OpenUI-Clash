// Modified for OpenUI (see OPENUI-PATCH.md): bounded drain of unread bodies, non-blocking on
// the connection's socket and with a deadline.
use std::io::Result as IoResult;
use std::io::{ErrorKind, Read};
use std::sync::mpsc::channel;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::connection::ConnectionControl;

/// A `Reader` that reads exactly the number of bytes from a sub-reader.
///
/// If the limit is reached, it returns EOF. If the limit is not reached
/// when the destructor is called, the remaining bytes will be read and
/// thrown away, within the bounds described at `DRAIN_LIMIT`.
pub struct EqualReader<R>
where
    R: Read,
{
    reader: R,
    size: usize,
    last_read_signal: Sender<IoResult<()>>,
    // The client connection the body arrives on; `None` for a body in memory.
    connection: Option<Arc<ConnectionControl>>,
}

impl<R> EqualReader<R>
where
    R: Read,
{
    pub fn new(
        reader: R,
        size: usize,
        connection: Option<Arc<ConnectionControl>>,
    ) -> (EqualReader<R>, Receiver<IoResult<()>>) {
        let (tx, rx) = channel();

        let r = EqualReader {
            reader,
            size,
            last_read_signal: tx,
            connection,
        };

        (r, rx)
    }
}

impl<R> Read for EqualReader<R>
where
    R: Read,
{
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        if self.size == 0 {
            return Ok(0);
        }

        let buf = if buf.len() < self.size {
            buf
        } else {
            &mut buf[..self.size]
        };

        match self.reader.read(buf) {
            Ok(len) => {
                self.size -= len;
                Ok(len)
            }
            err @ Err(_) => err,
        }
    }
}

/// Most of an unread body that is read and thrown away so the connection can serve its
/// next request; a longer body ends the connection.
const DRAIN_LIMIT: usize = 1024 * 1024;
/// The drain runs on the thread that drops the request, one of the application's
/// handlers, so it never waits for the client: on a connection it reads non-blocking,
/// taking only what has already arrived, and no read starts after `DRAIN_DEADLINE`. A
/// body that is not all there ends the connection.
const DRAIN_DEADLINE: Duration = Duration::from_secs(1);

impl<R> EqualReader<R>
where
    R: Read,
{
    /// Reads and throws away the rest of the body within `DRAIN_LIMIT` and
    /// `DRAIN_DEADLINE`; true if all of it was consumed. On a non-blocking socket a
    /// `WouldBlock` (nothing more has arrived) ends it like any other error.
    fn drain(&mut self) -> bool {
        // The original allocated the whole remaining Content-Length at once, so a request
        // claiming terabytes aborted the process before any byte was read.
        let deadline = Instant::now() + DRAIN_DEADLINE;
        let mut remaining_to_read = self.size.min(DRAIN_LIMIT);
        let mut buf = [0u8; 8 * 1024];

        while remaining_to_read > 0 {
            if Instant::now() >= deadline {
                return false;
            }
            let len = remaining_to_read.min(buf.len());

            match self.reader.read(&mut buf[..len]) {
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => {
                    self.last_read_signal.send(Err(e)).ok();
                    return false;
                }
                Ok(0) => {
                    self.last_read_signal.send(Ok(())).ok();
                    return false;
                }
                Ok(other) => {
                    remaining_to_read -= other;
                    self.size -= other;
                }
            }
        }
        self.size == 0
    }
}

impl<R> Drop for EqualReader<R>
where
    R: Read,
{
    fn drop(&mut self) {
        if self.size == 0 {
            return;
        }
        let connection = match self.connection.take() {
            Some(connection) => connection,
            None => {
                self.drain();
                return;
            }
        };
        // With blocking reads every read could wait for the connection timeout, renewed by
        // each byte the client trickles, while holding an application thread. Nothing else
        // reads the socket now (the next request waits for this reader to be dropped), and
        // this request's response is written already or not started yet (a body that ended
        // early, `Request::into_writer`), so the drain can be non-blocking. The flag is
        // shared by every handle on the socket: an earlier response still being written,
        // possible only with pipelining, which browsers do not use, can fail meanwhile.
        let reusable = connection.set_nonblocking(true).is_ok() && self.drain();
        let restored = connection.set_nonblocking(false).is_ok();
        if !(reusable && restored) {
            connection.abandon();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::EqualReader;
    use std::io::Read;

    #[test]
    fn test_limit() {
        use std::io::Cursor;

        let mut org_reader = Cursor::new("hello world".to_string().into_bytes());

        {
            let (mut equal_reader, _) = EqualReader::new(org_reader.by_ref(), 5, None);

            let mut string = String::new();
            equal_reader.read_to_string(&mut string).unwrap();
            assert_eq!(string, "hello");
        }

        let mut string = String::new();
        org_reader.read_to_string(&mut string).unwrap();
        assert_eq!(string, " world");
    }

    #[test]
    fn test_not_enough() {
        use std::io::Cursor;

        let mut org_reader = Cursor::new("hello world".to_string().into_bytes());

        {
            let (mut equal_reader, _) = EqualReader::new(org_reader.by_ref(), 5, None);

            let mut vec = [0];
            equal_reader.read_exact(&mut vec).unwrap();
            assert_eq!(vec[0], b'h');
        }

        let mut string = String::new();
        org_reader.read_to_string(&mut string).unwrap();
        assert_eq!(string, " world");
    }
}
