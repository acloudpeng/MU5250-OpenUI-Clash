// Modified for OpenUI (see OPENUI-PATCH.md): Connection::set_timeouts, set_read_timeout,
// set_nonblocking and read_shared; ConnectionControl with lingering close.
//! Abstractions of Tcp and Unix socket types

#[cfg(unix)]
use std::os::unix::net as unix_net;
use std::{
    io::{ErrorKind, Read},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

/// Unified listener. Either a [`TcpListener`] or [`std::os::unix::net::UnixListener`]
pub enum Listener {
    Tcp(TcpListener),
    #[cfg(unix)]
    Unix(unix_net::UnixListener),
}
impl Listener {
    pub(crate) fn local_addr(&self) -> std::io::Result<ListenAddr> {
        match self {
            Self::Tcp(l) => l.local_addr().map(ListenAddr::from),
            #[cfg(unix)]
            Self::Unix(l) => l.local_addr().map(ListenAddr::from),
        }
    }

    pub(crate) fn accept(&self) -> std::io::Result<(Connection, Option<SocketAddr>)> {
        match self {
            Self::Tcp(l) => l
                .accept()
                .map(|(conn, addr)| (Connection::from(conn), Some(addr))),
            #[cfg(unix)]
            Self::Unix(l) => l.accept().map(|(conn, _)| (Connection::from(conn), None)),
        }
    }
}
impl From<TcpListener> for Listener {
    fn from(s: TcpListener) -> Self {
        Self::Tcp(s)
    }
}
#[cfg(unix)]
impl From<unix_net::UnixListener> for Listener {
    fn from(s: unix_net::UnixListener) -> Self {
        Self::Unix(s)
    }
}

/// Unified connection. Either a [`TcpStream`] or [`std::os::unix::net::UnixStream`].
#[derive(Debug)]
pub(crate) enum Connection {
    Tcp(TcpStream),
    #[cfg(unix)]
    Unix(unix_net::UnixStream),
}
impl std::io::Read for Connection {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Tcp(s) => s.read(buf),
            #[cfg(unix)]
            Self::Unix(s) => s.read(buf),
        }
    }
}
impl std::io::Write for Connection {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Tcp(s) => s.write(buf),
            #[cfg(unix)]
            Self::Unix(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Tcp(s) => s.flush(),
            #[cfg(unix)]
            Self::Unix(s) => s.flush(),
        }
    }
}
impl Connection {
    /// Gets the peer's address. Some for TCP, None for Unix sockets.
    pub(crate) fn peer_addr(&mut self) -> std::io::Result<Option<SocketAddr>> {
        match self {
            Self::Tcp(s) => s.peer_addr().map(Some),
            #[cfg(unix)]
            Self::Unix(_) => Ok(None),
        }
    }

    /// Bounds how long one read or write may block, so a client that stops sending
    /// (or vanished without closing) frees its threads instead of holding them forever.
    pub(crate) fn set_timeouts(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        match self {
            Self::Tcp(s) => {
                s.set_read_timeout(timeout)?;
                s.set_write_timeout(timeout)
            }
            #[cfg(unix)]
            Self::Unix(s) => {
                s.set_read_timeout(timeout)?;
                s.set_write_timeout(timeout)
            }
        }
    }

    /// Changes only the read timeout; the socket option is shared by every handle on it.
    pub(crate) fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        match self {
            Self::Tcp(s) => s.set_read_timeout(timeout),
            #[cfg(unix)]
            Self::Unix(s) => s.set_read_timeout(timeout),
        }
    }

    /// Reads through a shared reference (the socket types implement `Read` for `&Self`).
    pub(crate) fn read_shared(&self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Tcp(s) => (&*s).read(buf),
            #[cfg(unix)]
            Self::Unix(s) => (&*s).read(buf),
        }
    }

    pub(crate) fn set_nonblocking(&self, nonblocking: bool) -> std::io::Result<()> {
        match self {
            Self::Tcp(s) => s.set_nonblocking(nonblocking),
            #[cfg(unix)]
            Self::Unix(s) => s.set_nonblocking(nonblocking),
        }
    }

    pub(crate) fn shutdown(&self, how: Shutdown) -> std::io::Result<()> {
        match self {
            Self::Tcp(s) => s.shutdown(how),
            #[cfg(unix)]
            Self::Unix(s) => s.shutdown(how),
        }
    }

    pub(crate) fn try_clone(&self) -> std::io::Result<Self> {
        match self {
            Self::Tcp(s) => s.try_clone().map(Self::from),
            #[cfg(unix)]
            Self::Unix(s) => s.try_clone().map(Self::from),
        }
    }
}

/// A client connection as its requests' body readers need it, shared by those requests:
/// a handle on the socket (under TLS too), so that a body left unread can be drained
/// without waiting for the client, and a way to give up on the connection when it cannot
/// be.
pub(crate) struct ConnectionControl {
    socket: Connection,
    abandoned: AtomicBool,
}

impl ConnectionControl {
    pub(crate) fn new(socket: Connection) -> Self {
        ConnectionControl {
            socket,
            abandoned: AtomicBool::new(false),
        }
    }

    /// Switches the socket (every handle on it) between blocking and non-blocking reads.
    pub(crate) fn set_nonblocking(&self, nonblocking: bool) -> std::io::Result<()> {
        self.socket.set_nonblocking(nonblocking)
    }

    /// Gives up on the connection after a request body on it was left part-way. What
    /// follows is the rest of that body, so the connection must never parse it: the
    /// flag makes its reads fail (a read shutdown would not do, Linux still returns
    /// bytes, queued or new, after one), and the connection's thread then lingers and
    /// lets go of the socket, which closes once the responses on it are written.
    pub(crate) fn abandon(&self) {
        self.abandoned.store(true, Ordering::Release);
    }

    /// Ends an abandoned connection, on its own thread: reads and throws away what the
    /// client is still sending for a short while before this thread lets go of the socket.
    /// Closing with unread data resets the connection, which can make the client lose the
    /// response it has not read yet (as with an upload refused early). It does not shut
    /// down writing: a body that ended early is dropped (by `FusedReader`) while the
    /// application is still reading it, before its response is written, and the socket
    /// only closes, FIN after the response, once the request's writer lets go too. For
    /// the same reason only the read timeout is shortened: that response may still be
    /// written, and keeps the connection's write timeout.
    pub(crate) fn linger(&self) {
        const QUIET: Duration = Duration::from_millis(200);
        const LINGER: Duration = Duration::from_secs(2);
        const LIMIT: usize = 16 * 1024 * 1024;
        if self.socket.set_read_timeout(Some(QUIET)).is_err() {
            return;
        }
        let deadline = Instant::now() + LINGER;
        let mut buf = [0u8; 8 * 1024];
        let mut discarded = 0;
        while discarded < LIMIT && Instant::now() < deadline {
            match self.socket.read_shared(&mut buf) {
                Ok(0) => return,
                Ok(n) => discarded += n,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => return,
            }
        }
    }

    pub(crate) fn is_abandoned(&self) -> bool {
        self.abandoned.load(Ordering::Acquire)
    }
}
impl From<TcpStream> for Connection {
    fn from(s: TcpStream) -> Self {
        Self::Tcp(s)
    }
}
#[cfg(unix)]
impl From<unix_net::UnixStream> for Connection {
    fn from(s: unix_net::UnixStream) -> Self {
        Self::Unix(s)
    }
}

#[derive(Debug, Clone)]
pub enum ConfigListenAddr {
    IP(Vec<SocketAddr>),
    #[cfg(unix)]
    // TODO: use SocketAddr when bind_addr is stabilized
    Unix(std::path::PathBuf),
}
impl ConfigListenAddr {
    pub fn from_socket_addrs<A: ToSocketAddrs>(addrs: A) -> std::io::Result<Self> {
        addrs.to_socket_addrs().map(|it| Self::IP(it.collect()))
    }

    #[cfg(unix)]
    pub fn unix_from_path<P: Into<PathBuf>>(path: P) -> Self {
        Self::Unix(path.into())
    }

    pub(crate) fn bind(&self) -> std::io::Result<Listener> {
        match self {
            Self::IP(a) => TcpListener::bind(a.as_slice()).map(Listener::from),
            #[cfg(unix)]
            Self::Unix(a) => unix_net::UnixListener::bind(a).map(Listener::from),
        }
    }
}

/// Unified listen socket address. Either a [`SocketAddr`] or [`std::os::unix::net::SocketAddr`].
#[derive(Debug, Clone)]
pub enum ListenAddr {
    IP(SocketAddr),
    #[cfg(unix)]
    Unix(unix_net::SocketAddr),
}
impl ListenAddr {
    pub fn to_ip(self) -> Option<SocketAddr> {
        match self {
            Self::IP(s) => Some(s),
            #[cfg(unix)]
            Self::Unix(_) => None,
        }
    }

    /// Gets the Unix socket address.
    ///
    /// This is also available on non-Unix platforms, for ease of use, but always returns `None`.
    #[cfg(unix)]
    pub fn to_unix(self) -> Option<unix_net::SocketAddr> {
        match self {
            Self::IP(_) => None,
            Self::Unix(s) => Some(s),
        }
    }
    #[cfg(not(unix))]
    pub fn to_unix(self) -> Option<SocketAddr> {
        None
    }
}
impl From<SocketAddr> for ListenAddr {
    fn from(s: SocketAddr) -> Self {
        Self::IP(s)
    }
}
#[cfg(unix)]
impl From<unix_net::SocketAddr> for ListenAddr {
    fn from(s: unix_net::SocketAddr) -> Self {
        Self::Unix(s)
    }
}
impl std::fmt::Display for ListenAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IP(s) => s.fmt(f),
            #[cfg(unix)]
            Self::Unix(s) => std::fmt::Debug::fmt(s, f),
        }
    }
}
