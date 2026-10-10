//! QMI requests to the modem's own services over QRTR (AF_QIPCRTR datagrams): UIM for
//! SIM access (logical channels, APDUs, slot status, card power) and DMS for the radio
//! operating mode. The firmware's daemons are QMI clients of the same services; each
//! socket here is a separate client, which the modem forgets when the socket closes.
//! Message IDs and TLV layouts follow libqmi's service definitions
//! (data/qmi-service-uim.json and data/qmi-service-dms.json).
use std::fmt;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

pub const SERVICE_DMS: u32 = 2;
pub const SERVICE_UIM: u32 = 11;

const UIM_GET_CARD_STATUS: u16 = 0x002F;
const UIM_POWER_OFF_SIM: u16 = 0x0030;
const UIM_POWER_ON_SIM: u16 = 0x0031;
const UIM_SEND_APDU: u16 = 0x003B;
const UIM_LOGICAL_CHANNEL: u16 = 0x003F;
const UIM_OPEN_LOGICAL_CHANNEL: u16 = 0x0042;
const UIM_GET_SLOT_STATUS: u16 = 0x0047;
const DMS_GET_OPERATING_MODE: u16 = 0x002D;
const DMS_SET_OPERATING_MODE: u16 = 0x002E;

pub const MODE_ONLINE: u8 = 0;
pub const MODE_LOW_POWER: u8 = 1;

const AF_QIPCRTR: libc::c_int = 42;
const QRTR_PORT_CTRL: u32 = 0xffff_fffe;
const QRTR_TYPE_NEW_SERVER: u32 = 4;
const QRTR_TYPE_NEW_LOOKUP: u32 = 10;

const MSG_REQUEST: u8 = 0x00;
const MSG_RESPONSE: u8 = 0x02;
const TLV_RESULT: u8 = 0x02;
/// Largest datagram read; QMI replies here stay far below it.
const MAX_MESSAGE: usize = 16 * 1024;

/// Debug builds only: QMI over Unix datagram sockets `<dir>/qmi-<service>.sock`, so the
/// end-to-end tests can stand in for the modem on a kernel without QRTR.
#[cfg(debug_assertions)]
pub const TEST_SOCKET_DIR: &str = "OPENUI_QMI_SOCKET_DIR";

#[derive(Debug)]
pub enum Error {
    /// No QRTR in the kernel, or the modem does not offer the service.
    Unavailable,
    Timeout,
    Io(io::Error),
    Malformed,
    /// The service answered with a failure result.
    Refused {
        code: u16,
        tlvs: Vec<Tlv>,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unavailable => f.write_str("The modem's SIM service is not reachable"),
            Error::Timeout => f.write_str("The modem did not answer in time"),
            Error::Io(e) => write!(f, "Modem communication failed: {e}"),
            Error::Malformed => f.write_str("The modem sent an unexpected reply"),
            Error::Refused { code, .. } => {
                write!(f, "The modem refused the request (QMI error {code})")
            }
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => Error::Timeout,
            _ => Error::Io(e),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tlv {
    pub kind: u8,
    pub value: Vec<u8>,
}

fn tlv(kind: u8, value: Vec<u8>) -> Tlv {
    Tlv { kind, value }
}

pub fn find(tlvs: &[Tlv], kind: u8) -> Option<&[u8]> {
    tlvs.iter()
        .find(|t| t.kind == kind)
        .map(|t| t.value.as_slice())
}

/// One QMI service message: flags, transaction, message ID, TLV length, TLVs.
fn encode(flags: u8, txn: u16, msg: u16, tlvs: &[Tlv]) -> Vec<u8> {
    let body: usize = tlvs.iter().map(|t| 3 + t.value.len()).sum();
    let mut out = Vec::with_capacity(7 + body);
    out.push(flags);
    out.extend_from_slice(&txn.to_le_bytes());
    out.extend_from_slice(&msg.to_le_bytes());
    out.extend_from_slice(&(body as u16).to_le_bytes());
    for t in tlvs {
        out.push(t.kind);
        out.extend_from_slice(&(t.value.len() as u16).to_le_bytes());
        out.extend_from_slice(&t.value);
    }
    out
}

struct Message {
    flags: u8,
    txn: u16,
    msg: u16,
    tlvs: Vec<Tlv>,
}

fn decode(buf: &[u8]) -> Option<Message> {
    let mut r = Reader::new(buf);
    let flags = r.u8()?;
    let txn = r.u16()?;
    let msg = r.u16()?;
    let len = r.u16()? as usize;
    let mut body = Reader::new(r.bytes(len)?);
    let mut tlvs = Vec::new();
    while !body.done() {
        let kind = body.u8()?;
        let n = body.u16()? as usize;
        tlvs.push(tlv(kind, body.bytes(n)?.to_vec()));
    }
    Some(Message {
        flags,
        txn,
        msg,
        tlvs,
    })
}

/// Bounds-checked little-endian reads.
struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    fn done(&self) -> bool {
        self.pos >= self.buf.len()
    }
    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let out = self.buf.get(self.pos..end)?;
        self.pos = end;
        Some(out)
    }
    fn u8(&mut self) -> Option<u8> {
        self.bytes(1).map(|b| b[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.bytes(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        self.bytes(4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    /// A byte string with a one-byte length prefix.
    fn short(&mut self) -> Option<&'a [u8]> {
        let n = self.u8()? as usize;
        self.bytes(n)
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SockaddrQrtr {
    sq_family: libc::sa_family_t,
    sq_node: u32,
    sq_port: u32,
}

/// The name-service control packet: command, then service, instance, node and port.
fn control(cmd: u32, service: u32) -> [u8; 20] {
    let mut out = [0u8; 20];
    out[0..4].copy_from_slice(&cmd.to_le_bytes());
    out[4..8].copy_from_slice(&service.to_le_bytes());
    out
}

/// A NEW_SERVER announcement as (service, node, port); all zero ends a lookup's listing.
fn parse_server(buf: &[u8]) -> Option<(u32, u32, u32)> {
    let mut r = Reader::new(buf);
    if r.u32()? != QRTR_TYPE_NEW_SERVER {
        return None;
    }
    let service = r.u32()?;
    let _instance = r.u32()?;
    Some((service, r.u32()?, r.u32()?))
}

enum Link {
    Qrtr {
        fd: OwnedFd,
        node: u32,
        port: u32,
    },
    #[cfg(debug_assertions)]
    Unix {
        socket: std::os::unix::net::UnixDatagram,
        peer: std::path::PathBuf,
        own: std::path::PathBuf,
    },
}

#[cfg(debug_assertions)]
impl Drop for Link {
    fn drop(&mut self) {
        if let Link::Unix { own, .. } = self {
            let _ = std::fs::remove_file(own);
        }
    }
}

fn remaining(deadline: Instant) -> Result<Duration, Error> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or(Error::Timeout)
}

/// Waits for a datagram; `Error::Timeout` when the deadline passes first.
fn wait_readable(fd: libc::c_int, deadline: Instant) -> Result<(), Error> {
    loop {
        let ms = remaining(deadline)?.as_millis().clamp(1, i32::MAX as u128) as i32;
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let n = unsafe { libc::poll(&mut pfd, 1, ms) };
        if n > 0 {
            return Ok(());
        }
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(Error::Io(e));
            }
        }
    }
}

fn qrtr_recv(
    fd: libc::c_int,
    buf: &mut [u8],
    deadline: Instant,
) -> Result<(usize, u32, u32), Error> {
    wait_readable(fd, deadline)?;
    let mut from = SockaddrQrtr {
        sq_family: 0,
        sq_node: 0,
        sq_port: 0,
    };
    let mut len = std::mem::size_of::<SockaddrQrtr>() as libc::socklen_t;
    let n = unsafe {
        libc::recvfrom(
            fd,
            buf.as_mut_ptr().cast(),
            buf.len(),
            0,
            (&mut from as *mut SockaddrQrtr).cast(),
            &mut len,
        )
    };
    if n < 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok((n as usize, from.sq_node, from.sq_port))
}

fn qrtr_send(fd: libc::c_int, node: u32, port: u32, data: &[u8]) -> Result<(), Error> {
    let to = SockaddrQrtr {
        sq_family: AF_QIPCRTR as libc::sa_family_t,
        sq_node: node,
        sq_port: port,
    };
    let n = unsafe {
        libc::sendto(
            fd,
            data.as_ptr().cast(),
            data.len(),
            0,
            (&to as *const SockaddrQrtr).cast(),
            std::mem::size_of::<SockaddrQrtr>() as libc::socklen_t,
        )
    };
    if n < 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}

/// Opens a QRTR socket and asks the name service where `service` lives.
fn qrtr_connect(service: u32, deadline: Instant) -> Result<Link, Error> {
    let raw = unsafe { libc::socket(AF_QIPCRTR, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
    if raw < 0 {
        let e = io::Error::last_os_error();
        return Err(match e.raw_os_error() {
            Some(libc::EAFNOSUPPORT) | Some(libc::EPROTONOSUPPORT) => Error::Unavailable,
            _ => Error::Io(e),
        });
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let mut local = SockaddrQrtr {
        sq_family: 0,
        sq_node: 0,
        sq_port: 0,
    };
    let mut len = std::mem::size_of::<SockaddrQrtr>() as libc::socklen_t;
    if unsafe { libc::getsockname(raw, (&mut local as *mut SockaddrQrtr).cast(), &mut len) } < 0 {
        return Err(io::Error::last_os_error().into());
    }
    qrtr_send(
        raw,
        local.sq_node,
        QRTR_PORT_CTRL,
        &control(QRTR_TYPE_NEW_LOOKUP, service),
    )?;
    let mut buf = vec![0u8; 64];
    loop {
        let (n, _, from_port) = match qrtr_recv(raw, &mut buf, deadline) {
            Err(Error::Timeout) => return Err(Error::Unavailable),
            other => other?,
        };
        if from_port != QRTR_PORT_CTRL {
            continue;
        }
        match parse_server(&buf[..n]) {
            Some((0, 0, 0)) => return Err(Error::Unavailable),
            Some((s, node, port)) if s == service => return Ok(Link::Qrtr { fd, node, port }),
            _ => {}
        }
    }
}

#[cfg(debug_assertions)]
fn unix_connect(dir: &str, service: u32) -> Result<Link, Error> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = std::path::Path::new(dir);
    let own = dir.join(format!(
        "client-{}-{}.sock",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&own);
    let socket = std::os::unix::net::UnixDatagram::bind(&own)?;
    let peer = dir.join(format!("qmi-{service}.sock"));
    if !peer.exists() {
        let _ = std::fs::remove_file(&own);
        return Err(Error::Unavailable);
    }
    Ok(Link::Unix { socket, peer, own })
}

/// One QMI client of one service.
pub struct Client {
    link: Link,
    txn: u16,
}

impl Client {
    pub fn connect(service: u32, timeout: Duration) -> Result<Self, Error> {
        #[cfg(debug_assertions)]
        if let Ok(dir) = std::env::var(TEST_SOCKET_DIR) {
            return Ok(Self {
                link: unix_connect(&dir, service)?,
                txn: 0,
            });
        }
        Ok(Self {
            link: qrtr_connect(service, Instant::now() + timeout)?,
            txn: 0,
        })
    }

    fn send(&self, data: &[u8]) -> Result<(), Error> {
        match &self.link {
            Link::Qrtr { fd, node, port } => qrtr_send(fd.as_raw_fd(), *node, *port, data),
            #[cfg(debug_assertions)]
            Link::Unix { socket, peer, .. } => {
                socket.send_to(data, peer)?;
                Ok(())
            }
        }
    }

    /// The next datagram from the service itself; name-service notices are skipped.
    fn recv(&self, buf: &mut [u8], deadline: Instant) -> Result<usize, Error> {
        match &self.link {
            Link::Qrtr { fd, node, port } => loop {
                let (n, from_node, from_port) = qrtr_recv(fd.as_raw_fd(), buf, deadline)?;
                if from_node == *node && from_port == *port {
                    return Ok(n);
                }
            },
            #[cfg(debug_assertions)]
            Link::Unix { socket, .. } => {
                socket.set_read_timeout(Some(remaining(deadline)?))?;
                Ok(socket.recv(buf)?)
            }
        }
    }

    /// Sends a request and waits for its response; indications and stale replies are
    /// skipped. A failure result keeps the reply's TLVs (a card status, for example).
    pub fn request(
        &mut self,
        msg: u16,
        tlvs: &[Tlv],
        timeout: Duration,
    ) -> Result<Vec<Tlv>, Error> {
        self.txn = self.txn.wrapping_add(1).max(1);
        let deadline = Instant::now() + timeout;
        self.send(&encode(MSG_REQUEST, self.txn, msg, tlvs))?;
        let mut buf = vec![0u8; MAX_MESSAGE];
        loop {
            let n = self.recv(&mut buf, deadline)?;
            let Some(reply) = decode(&buf[..n]) else {
                continue;
            };
            if reply.flags != MSG_RESPONSE || reply.txn != self.txn || reply.msg != msg {
                continue;
            }
            let result = find(&reply.tlvs, TLV_RESULT).ok_or(Error::Malformed)?;
            let mut r = Reader::new(result);
            let (status, code) = (
                r.u16().ok_or(Error::Malformed)?,
                r.u16().ok_or(Error::Malformed)?,
            );
            if status != 0 {
                return Err(Error::Refused {
                    code,
                    tlvs: reply.tlvs,
                });
            }
            return Ok(reply.tlvs);
        }
    }
}

/// A status word from the card, e.g. 0x6A82 when the selected application is absent.
fn card_result(tlvs: &[Tlv]) -> Option<u16> {
    find(tlvs, 0x11)
        .filter(|v| v.len() >= 2)
        .map(|v| u16::from_be_bytes([v[0], v[1]]))
}

/// Why a logical channel could not be opened.
#[derive(Debug)]
pub enum OpenError {
    /// The card answered SELECT with this status word.
    Status(u16),
    Modem(Error),
}

/// QMI's "SIM file not found", what a modem may answer for an absent application
/// without passing on the card's status word.
const SIM_FILE_NOT_FOUND: u16 = 80;

/// A refused Open Logical Channel: the card's status word when there is one.
fn open_refusal(code: u16, tlvs: Vec<Tlv>) -> OpenError {
    match card_result(&tlvs) {
        Some(sw) if sw != 0x9000 => OpenError::Status(sw),
        _ if code == SIM_FILE_NOT_FOUND => OpenError::Status(0x6A82),
        _ => OpenError::Modem(Error::Refused { code, tlvs }),
    }
}

/// Request timeout for card commands; a profile install step can take seconds.
const APDU_TIMEOUT: Duration = Duration::from_secs(30);
const UIM_TIMEOUT: Duration = Duration::from_secs(10);

/// The UIM service for one logical slot (1-based, as QMI numbers them).
pub struct Uim {
    client: Client,
    slot: u8,
}

impl Uim {
    pub fn connect(slot: u8, timeout: Duration) -> Result<Self, Error> {
        Ok(Self {
            client: Client::connect(SERVICE_UIM, timeout)?,
            slot,
        })
    }

    /// Opens a logical channel and selects `aid` on it.
    pub fn open_channel(&mut self, aid: &[u8]) -> Result<u8, OpenError> {
        let mut value = vec![aid.len() as u8];
        value.extend_from_slice(aid);
        let request = [tlv(0x01, vec![self.slot]), tlv(0x10, value)];
        match self
            .client
            .request(UIM_OPEN_LOGICAL_CHANNEL, &request, UIM_TIMEOUT)
        {
            Ok(tlvs) => {
                let channel = find(&tlvs, 0x10)
                    .and_then(|v| v.first().copied())
                    .ok_or(OpenError::Modem(Error::Malformed))?;
                match card_result(&tlvs) {
                    Some(sw) if sw != 0x9000 && sw >> 8 != 0x61 => {
                        let _ = self.close_channel(channel);
                        Err(OpenError::Status(sw))
                    }
                    _ => Ok(channel),
                }
            }
            Err(Error::Refused { code, tlvs }) => Err(open_refusal(code, tlvs)),
            Err(e) => Err(OpenError::Modem(e)),
        }
    }

    /// Sends one command APDU on `channel`; the reply ends with its status word.
    pub fn transmit(&mut self, channel: u8, apdu: &[u8]) -> Result<Vec<u8>, Error> {
        let mut value = (apdu.len() as u16).to_le_bytes().to_vec();
        value.extend_from_slice(apdu);
        let request = [
            tlv(0x01, vec![self.slot]),
            tlv(0x02, value),
            tlv(0x10, vec![channel]),
        ];
        let tlvs = self.client.request(UIM_SEND_APDU, &request, APDU_TIMEOUT)?;
        let mut r = Reader::new(find(&tlvs, 0x10).ok_or(Error::Malformed)?);
        let n = r.u16().ok_or(Error::Malformed)? as usize;
        let response = r.bytes(n).ok_or(Error::Malformed)?;
        if response.len() < 2 {
            return Err(Error::Malformed);
        }
        Ok(response.to_vec())
    }

    pub fn close_channel(&mut self, channel: u8) -> Result<(), Error> {
        let request = [tlv(0x01, vec![self.slot]), tlv(0x11, vec![channel])];
        self.client
            .request(UIM_LOGICAL_CHANNEL, &request, UIM_TIMEOUT)
            .map(|_| ())
    }

    pub fn slot_status(&mut self) -> Result<Vec<PhysicalSlot>, Error> {
        let tlvs = self.client.request(UIM_GET_SLOT_STATUS, &[], UIM_TIMEOUT)?;
        parse_slots(&tlvs).ok_or(Error::Malformed)
    }

    pub fn card_status(&mut self) -> Result<Vec<Card>, Error> {
        let tlvs = self.client.request(UIM_GET_CARD_STATUS, &[], UIM_TIMEOUT)?;
        parse_cards(find(&tlvs, 0x10).ok_or(Error::Malformed)?).ok_or(Error::Malformed)
    }

    /// Powers the card off or on; the modem reads it afresh on power-up.
    pub fn power(&mut self, on: bool) -> Result<(), Error> {
        let msg = if on {
            UIM_POWER_ON_SIM
        } else {
            UIM_POWER_OFF_SIM
        };
        self.client
            .request(msg, &[tlv(0x01, vec![self.slot])], UIM_TIMEOUT)
            .map(|_| ())
    }
}

/// One physical slot from Get Slot Status.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PhysicalSlot {
    /// None when the modem does not know.
    pub present: Option<bool>,
    pub active: bool,
    pub logical: u8,
    pub iccid: String,
    pub euicc: Option<bool>,
    pub eid: String,
}

fn parse_slots(tlvs: &[Tlv]) -> Option<Vec<PhysicalSlot>> {
    let mut r = Reader::new(find(tlvs, 0x10)?);
    let mut slots = Vec::new();
    for _ in 0..r.u8()? {
        let card = r.u32()?;
        let state = r.u32()?;
        let logical = r.u8()?;
        let iccid = bcd(r.short()?);
        slots.push(PhysicalSlot {
            present: match card {
                1 => Some(false),
                2 => Some(true),
                _ => None,
            },
            active: state == 1,
            logical,
            iccid,
            euicc: None,
            eid: String::new(),
        });
    }
    if let Some(info) = find(tlvs, 0x11) {
        let mut r = Reader::new(info);
        let count = r.u8()? as usize;
        for slot in slots.iter_mut().take(count) {
            let _protocol = r.u32()?;
            let _apps = r.u8()?;
            let _atr = r.short()?;
            slot.euicc = Some(r.u8()? != 0);
        }
    }
    if let Some(eids) = find(tlvs, 0x12) {
        let mut r = Reader::new(eids);
        let count = r.u8()? as usize;
        for slot in slots.iter_mut().take(count) {
            slot.eid = hex_upper(r.short()?);
        }
    }
    Some(slots)
}

/// One card from Get Card Status, with its applications as (type, state).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    /// 0 absent, 1 present, 2 error.
    pub state: u8,
    pub error: u8,
    pub apps: Vec<(u8, u8)>,
}

impl Card {
    /// A USIM (type 2) in the ready state (7).
    pub fn usim_ready(&self) -> bool {
        self.state == 1
            && self
                .apps
                .iter()
                .any(|&(kind, state)| kind == 2 && state == 7)
    }
}

fn parse_cards(value: &[u8]) -> Option<Vec<Card>> {
    let mut r = Reader::new(value);
    r.bytes(8)?; // primary and secondary session indexes
    let mut cards = Vec::new();
    for _ in 0..r.u8()? {
        let state = r.u8()?;
        r.bytes(3)?; // UPIN state and retries
        let error = r.u8()?;
        let mut apps = Vec::new();
        for _ in 0..r.u8()? {
            let kind = r.u8()?;
            let app_state = r.u8()?;
            r.bytes(4)?; // personalization
            r.short()?; // AID
            r.bytes(7)?; // PIN states and retries
            apps.push((kind, app_state));
        }
        cards.push(Card { state, error, apps });
    }
    Some(cards)
}

/// Swapped-nibble BCD as in EF.ICCID, without the F padding.
pub fn bcd(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        for nibble in [b & 0x0F, b >> 4] {
            if nibble <= 9 {
                out.push((b'0' + nibble) as char);
            } else if nibble != 0x0F {
                out.push(
                    char::from_digit(nibble as u32, 16)
                        .unwrap_or('?')
                        .to_ascii_uppercase(),
                );
            }
        }
    }
    out
}

pub fn hex_upper(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// The radio operating mode (`MODE_ONLINE`, `MODE_LOW_POWER`, ...).
pub fn operating_mode(client: &mut Client) -> Result<u8, Error> {
    let tlvs = client.request(DMS_GET_OPERATING_MODE, &[], UIM_TIMEOUT)?;
    find(&tlvs, 0x01)
        .and_then(|v| v.first().copied())
        .ok_or(Error::Malformed)
}

pub fn set_operating_mode(client: &mut Client, mode: u8) -> Result<(), Error> {
    client
        .request(
            DMS_SET_OPERATING_MODE,
            &[tlv(0x01, vec![mode])],
            UIM_TIMEOUT,
        )
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_round_trip() {
        let bytes = encode(
            MSG_REQUEST,
            7,
            UIM_SEND_APDU,
            &[tlv(0x01, vec![1]), tlv(0x02, vec![2, 0, 0x80, 0xE2])],
        );
        assert_eq!(
            bytes,
            [0, 7, 0, 0x3B, 0, 11, 0, 1, 1, 0, 1, 2, 4, 0, 2, 0, 0x80, 0xE2]
        );
        let m = decode(&bytes).unwrap();
        assert_eq!((m.flags, m.txn, m.msg), (MSG_REQUEST, 7, UIM_SEND_APDU));
        assert_eq!(m.tlvs[1].value, [2, 0, 0x80, 0xE2]);
        // Truncated TLV and overlong length are rejected, not read past the end.
        assert!(decode(&bytes[..bytes.len() - 1]).is_none());
        assert!(decode(&[2, 1, 0, 0x3B, 0, 9, 0, 2, 4, 0]).is_none());
    }

    #[test]
    fn control_packets() {
        assert_eq!(
            control(QRTR_TYPE_NEW_LOOKUP, SERVICE_UIM),
            [10, 0, 0, 0, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        let mut server = [0u8; 20];
        server[0] = 4;
        server[4] = 11;
        server[8] = 1;
        server[12] = 0;
        server[16] = 0x2a;
        assert_eq!(parse_server(&server), Some((11, 0, 0x2a)));
        assert_eq!(parse_server(&[0u8; 20]), None);
        assert_eq!(parse_server(&server[..16]), None);
        let mut end = [0u8; 20];
        end[0] = 4;
        assert_eq!(parse_server(&end), Some((0, 0, 0)));
    }

    #[test]
    fn iccid_and_eid_digits() {
        assert_eq!(
            bcd(&[0x98, 0x68, 0x00, 0x01, 0x21, 0x43, 0x65, 0x87, 0x09, 0xF1]),
            "8986001012345678901"
        );
        assert_eq!(bcd(&[0x98, 0x10, 0x32]), "890123");
        assert_eq!(hex_upper(&[0x89, 0x04, 0xab]), "8904AB");
    }

    #[test]
    fn slot_status() {
        let mut v = vec![2];
        // Slot 1: present, active, logical 1, ICCID.
        v.extend_from_slice(&2u32.to_le_bytes());
        v.extend_from_slice(&1u32.to_le_bytes());
        v.extend_from_slice(&[1, 3, 0x98, 0x10, 0x32]);
        // Slot 2: absent, inactive, unmapped.
        v.extend_from_slice(&1u32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&[0, 0]);
        let mut info = vec![1];
        info.extend_from_slice(&1u32.to_le_bytes());
        info.extend_from_slice(&[1, 2, 0x3B, 0x9F, 1]);
        let eid = [vec![1, 2, 0x89, 0x04]].concat();
        let slots = parse_slots(&[tlv(0x10, v), tlv(0x11, info), tlv(0x12, eid)]).unwrap();
        assert_eq!(slots.len(), 2);
        assert_eq!(
            slots[0],
            PhysicalSlot {
                present: Some(true),
                active: true,
                logical: 1,
                iccid: "890123".into(),
                euicc: Some(true),
                eid: "8904".into(),
            }
        );
        assert_eq!(slots[1].present, Some(false));
        assert!(!slots[1].active);
        assert_eq!(slots[1].euicc, None);
        assert!(parse_slots(&[tlv(0x10, vec![1, 2, 0])]).is_none());
    }

    #[test]
    fn card_status_usim_ready() {
        let mut v = vec![0, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 1];
        v.extend_from_slice(&[1, 0, 3, 10, 0, 2]);
        // USIM ready with a 2-byte AID, then ISIM detected without one.
        v.extend_from_slice(&[2, 7, 0, 0, 0, 0, 2, 0xA0, 0x00, 0, 1, 3, 10, 0, 3, 10]);
        v.extend_from_slice(&[5, 1, 0, 0, 0, 0, 0, 0, 1, 3, 10, 0, 3, 10]);
        let cards = parse_cards(&v).unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].apps, [(2, 7), (5, 1)]);
        assert!(cards[0].usim_ready());
        let mut absent = v.clone();
        absent[9] = 0;
        assert!(!parse_cards(&absent).unwrap()[0].usim_ready());
        assert!(parse_cards(&v[..v.len() - 3]).is_none());
    }

    #[test]
    fn select_failure_status() {
        assert_eq!(card_result(&[tlv(0x11, vec![0x6A, 0x82])]), Some(0x6A82));
        assert_eq!(card_result(&[tlv(0x11, vec![0x6A])]), None);
        assert_eq!(card_result(&[]), None);
        let status = |e: OpenError| match e {
            OpenError::Status(sw) => Some(sw),
            OpenError::Modem(_) => None,
        };
        // The card's own word wins; without one, "SIM file not found" means no ISD-R.
        assert_eq!(
            status(open_refusal(3, vec![tlv(0x11, vec![0x6A, 0x82])])),
            Some(0x6A82)
        );
        assert_eq!(status(open_refusal(80, vec![])), Some(0x6A82));
        assert_eq!(
            status(open_refusal(80, vec![tlv(0x11, vec![0x69, 0x85])])),
            Some(0x6985)
        );
        assert_eq!(status(open_refusal(52, vec![])), None);
    }
}
