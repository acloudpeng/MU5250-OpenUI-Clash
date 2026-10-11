//! Non-exclusive evdev observer of the touchscreen. Two fingers held still for two seconds
//! on the lit stock UI request the existing supervisor, as the four-press power shortcut
//! does, without wearing the power button. The device is never grabbed: the stock UI keeps
//! receiving every touch through `mtdev2tuio`.
use crate::screen_shortcut::InputEvent;
use crate::{screen, screen_settings};
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    os::unix::{fs::OpenOptionsExt, io::AsRawFd},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};
const HOLD_MS: i64 = 2000;
/// A finger moving further than this share of an axis cancels the hold (a swipe or scroll).
const DRIFT: i32 = 12;
const SLOTS: usize = 10;
const BACKLIGHT: &str = "/sys/class/leds/led:lcd/brightness";
const ABS_MT_SLOT: u16 = 0x2f;
const ABS_MT_POSITION_X: u16 = 0x35;
const ABS_MT_POSITION_Y: u16 = 0x36;
const ABS_MT_TRACKING_ID: u16 = 0x39;
static AVAILABLE: AtomicBool = AtomicBool::new(false);
static RESULT: Mutex<&str> = Mutex::new("Waiting for the touchscreen");
#[derive(Clone, Copy, Default)]
struct Contact {
    active: bool,
    x: Option<i32>,
    y: Option<i32>,
    start: Option<(i32, i32)>,
}
/// Multi-touch protocol B, reduced to "exactly two fingers, down and still".
#[derive(Default)]
struct Hold {
    slot: usize,
    contacts: [Contact; SLOTS],
    /// A frame without contacts has been seen. Fingers already down when the observer
    /// opened, or across dropped events, never count: every finger must lift first.
    armed: bool,
    dropped: bool,
    /// Dropped events just ended; the device must be read again (`resync`).
    stale: bool,
    /// Since when exactly two fingers have been down and still.
    since: Option<i64>,
    /// This touch fired or was disqualified; every finger must lift first.
    spent: bool,
    /// Axis spans for the drift limit; 0 when the driver does not report one.
    span: (i32, i32),
}
impl Hold {
    fn fingers(&self) -> usize {
        self.contacts.iter().filter(|c| c.active).count()
    }
    /// One input event; `ms` is the observer's monotonic clock, since a held finger sends
    /// nothing and `due` is checked against the same clock.
    fn event(&mut self, kind: u16, code: u16, value: i32, ms: i64) {
        match (kind, code) {
            // SYN_DROPPED: discard everything up to and including the next SYN_REPORT.
            (0, 3) => {
                self.contacts = Default::default();
                self.armed = false;
                self.dropped = true;
                self.since = None;
            }
            (0, 0) if self.dropped => {
                self.dropped = false;
                self.stale = true;
            }
            _ if self.dropped => {}
            (3, ABS_MT_SLOT) => self.slot = usize::try_from(value).unwrap_or(SLOTS),
            (3, ABS_MT_TRACKING_ID) => {
                if let Some(c) = self.contacts.get_mut(self.slot) {
                    *c = Contact {
                        active: value >= 0,
                        ..Contact::default()
                    };
                }
            }
            (3, ABS_MT_POSITION_X) => {
                if let Some(c) = self.contacts.get_mut(self.slot) {
                    c.x = Some(value);
                }
            }
            (3, ABS_MT_POSITION_Y) => {
                if let Some(c) = self.contacts.get_mut(self.slot) {
                    c.y = Some(value);
                }
            }
            (0, 0) => self.frame(ms),
            _ => {}
        }
    }
    fn frame(&mut self, ms: i64) {
        for c in self
            .contacts
            .iter_mut()
            .filter(|c| c.active && c.start.is_none())
        {
            if let (Some(x), Some(y)) = (c.x, c.y) {
                c.start = Some((x, y));
            }
        }
        let fingers = self.fingers();
        if fingers == 0 {
            self.armed = true;
            self.spent = false;
            self.since = None;
            return;
        }
        if !self.armed || self.spent {
            self.since = None;
            return;
        }
        let moved = self.contacts.iter().any(|c| c.active && self.drifted(c));
        if fingers > 2 || moved {
            self.spent = true;
            self.since = None;
        } else if fingers == 2 {
            self.since.get_or_insert(ms);
        } else {
            self.since = None;
        }
    }
    fn drifted(&self, c: &Contact) -> bool {
        let (Some((x0, y0)), Some(x), Some(y)) = (c.start, c.x, c.y) else {
            return false;
        };
        let far = |d: i32, span: i32| span > 0 && d.saturating_abs().saturating_mul(DRIFT) > span;
        far(x.saturating_sub(x0), self.span.0) || far(y.saturating_sub(y0), self.span.1)
    }
    /// Two fingers are down and still, and the hold is running.
    fn holding(&self) -> bool {
        self.since.is_some()
    }
    /// True once two fingers have stayed down and still for `HOLD_MS`; the touch is then spent.
    fn due(&mut self, ms: i64) -> bool {
        let held = self.since.is_some_and(|t| ms - t >= HOLD_MS);
        if held && !self.spent && self.fingers() == 2 {
            self.spent = true;
            self.since = None;
            return true;
        }
        false
    }
    /// The stream alone cannot tell which fingers are down after a fresh open or dropped
    /// events, so the device's slots are read again. Fingers found down keep the hold
    /// unarmed until every one has lifted.
    fn resync(&mut self, slot: usize, ids: [i32; SLOTS]) {
        self.slot = slot;
        for (c, id) in self.contacts.iter_mut().zip(ids) {
            *c = Contact {
                active: id >= 0,
                ..Contact::default()
            };
        }
        self.armed = self.fingers() == 0;
        self.since = None;
    }
    /// While the hold is off or DevUI is up, a touch in progress may not fire afterwards.
    fn hold_off(&mut self) {
        if self.fingers() > 0 {
            self.spent = true;
        }
        self.since = None;
    }
}
/// EVIOCGABS: value, minimum, maximum, fuzz, flat and resolution of an axis.
fn absinfo(file: &File, axis: u16) -> Option<[i32; 6]> {
    let mut info = [0i32; 6];
    let size = std::mem::size_of_val(&info) as u32;
    let request = (2u32 << 30) | (size << 16) | (u32::from(b'E') << 8) | (0x40 + u32::from(axis));
    // SAFETY: EVIOCGABS writes one struct input_absinfo, six i32 values, into `info`.
    let ok =
        unsafe { libc::ioctl(file.as_raw_fd(), request as libc::Ioctl, info.as_mut_ptr()) } == 0;
    ok.then_some(info)
}
/// The span of an axis, or 0 when the driver does not report it.
fn axis_span(file: &File, axis: u16) -> i32 {
    absinfo(file, axis).map_or(0, |i| i[2].saturating_sub(i[1]).max(0))
}
/// The current slot and every slot's tracking ID (-1 when no finger is down there).
fn slots(file: &File) -> Option<(usize, [i32; SLOTS])> {
    // struct input_mt_request_layout: the code, then one value per slot.
    let mut layout = [-1i32; SLOTS + 1];
    layout[0] = i32::from(ABS_MT_TRACKING_ID);
    let size = std::mem::size_of_val(&layout) as u32;
    let request = (2u32 << 30) | (size << 16) | (u32::from(b'E') << 8) | 0x0a;
    // SAFETY: EVIOCGMTSLOTS reads the code from `layout[0]` and writes at most one value
    // per slot after it, never more than the size encoded in the request.
    let ok = unsafe {
        libc::ioctl(
            file.as_raw_fd(),
            request as libc::Ioctl,
            layout.as_mut_ptr(),
        )
    } == 0;
    let slot = usize::try_from(absinfo(file, ABS_MT_SLOT)?[0]).ok()?;
    let mut ids = [-1; SLOTS];
    ids.copy_from_slice(&layout[1..]);
    ok.then_some((slot, ids))
}
fn open_touch() -> Option<(File, (i32, i32))> {
    (0..32).find_map(|i| {
        let name = fs::read_to_string(format!("/sys/class/input/event{i}/device/name")).ok()?;
        if !name.contains("sitronix") {
            return None;
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(format!("/dev/input/event{i}"))
            .ok()?;
        let span = (
            axis_span(&file, ABS_MT_POSITION_X),
            axis_span(&file, ABS_MT_POSITION_Y),
        );
        Some((file, span))
    })
}
/// Only a lit screen counts: a hold in a pocket must not open DevUI.
fn lit() -> bool {
    fs::read_to_string(BACKLIGHT)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .is_some_and(|v| v > 0)
}
pub fn status() -> Value {
    json!({"available":AVAILABLE.load(Ordering::Relaxed),"message":*RESULT.lock().unwrap()})
}
pub fn start() {
    std::thread::spawn(|| {
        let clock = Instant::now();
        let mut touch: Option<File> = None;
        let mut hold = Hold::default();
        let mut cooldown = 0;
        loop {
            if touch.is_none() {
                hold = Hold::default();
                match open_touch() {
                    Some((file, span)) => {
                        hold.span = span;
                        if let Some((slot, ids)) = slots(&file) {
                            hold.resync(slot, ids);
                        }
                        touch = Some(file);
                        AVAILABLE.store(true, Ordering::Relaxed);
                    }
                    None => {
                        AVAILABLE.store(false, Ordering::Relaxed);
                        std::thread::sleep(Duration::from_secs(2));
                        continue;
                    }
                }
            }
            let ms = i64::try_from(clock.elapsed().as_millis()).unwrap_or(i64::MAX);
            let mut failed = false;
            // Always drain, also while off or while DevUI owns the screen: turning the hold
            // on must never replay old touches.
            for _ in 0..256 {
                let mut bytes = [0u8; std::mem::size_of::<InputEvent>()];
                let read = touch.as_mut().unwrap().read(&mut bytes);
                match read {
                    Ok(n) if n == bytes.len() => {
                        let event = unsafe {
                            std::ptr::read_unaligned(bytes.as_ptr().cast::<InputEvent>())
                        };
                        hold.event(event.kind, event.code, event.value, ms);
                        if std::mem::take(&mut hold.stale) {
                            if let Some((slot, ids)) = touch.as_ref().and_then(slots) {
                                hold.resync(slot, ids);
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    _ => {
                        failed = true;
                        break;
                    }
                }
            }
            if failed {
                touch = None;
                AVAILABLE.store(false, Ordering::Relaxed);
                *RESULT.lock().unwrap() = "Touchscreen disconnected";
                continue;
            }
            let enabled = screen_settings::current().touch_enabled;
            // The screen stays lit for the whole hold: a hold that began in the dark does
            // not count once touch wake-up has lit the screen under the fingers.
            if !enabled || screen::running() || (hold.holding() && !lit()) {
                hold.hold_off();
            } else if hold.due(ms) && ms >= cooldown {
                cooldown = ms + 5000;
                let (code, _) = screen::open();
                *RESULT.lock().unwrap() = if code == 202 {
                    "Two-finger hold requested DevUI"
                } else {
                    "Hold blocked: check screen recovery status"
                };
            }
            {
                let mut result = RESULT.lock().unwrap();
                if !enabled {
                    *result = "Two-finger hold disabled";
                } else if matches!(
                    *result,
                    "Two-finger hold disabled"
                        | "Waiting for the touchscreen"
                        | "Touchscreen disconnected"
                ) {
                    *result = "Ready: hold two fingers still for 2 seconds";
                }
            }
            // Sleep until the next touch event. Held fingers send nothing, so while two
            // are down the time and the backlight are checked every 20 ms instead.
            let mut wait = libc::pollfd {
                fd: touch.as_ref().map_or(-1, |f| f.as_raw_fd()),
                events: libc::POLLIN,
                revents: 0,
            };
            let timeout = if hold.holding() { 20 } else { 1000 };
            // SAFETY: one pollfd, valid for the duration of the call.
            unsafe { libc::poll(&mut wait, 1, timeout) };
        }
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    fn finger(h: &mut Hold, slot: i32, id: i32, at: Option<(i32, i32)>) {
        h.event(3, ABS_MT_SLOT, slot, 0);
        h.event(3, ABS_MT_TRACKING_ID, id, 0);
        if let Some((x, y)) = at {
            h.event(3, ABS_MT_POSITION_X, x, 0);
            h.event(3, ABS_MT_POSITION_Y, y, 0);
        }
    }
    fn sync(h: &mut Hold, ms: i64) {
        h.event(0, 0, 0, ms);
    }
    /// An observer that has seen the screen untouched, on a 0..1000 square panel.
    fn armed() -> Hold {
        let mut h = Hold {
            span: (1000, 1000),
            ..Hold::default()
        };
        sync(&mut h, 0);
        h
    }
    fn two_fingers(h: &mut Hold, ms: i64) {
        finger(h, 0, 7, Some((300, 400)));
        finger(h, 1, 8, Some((600, 450)));
        sync(h, ms);
    }
    fn lift(h: &mut Hold, ms: i64) {
        finger(h, 0, -1, None);
        finger(h, 1, -1, None);
        sync(h, ms);
    }
    #[test]
    fn two_still_fingers_fire_once_per_touch() {
        let mut h = armed();
        two_fingers(&mut h, 1000);
        assert!(!h.due(2999));
        assert!(h.due(3000));
        assert!(!h.due(5000), "a touch fires once");
        lift(&mut h, 5100);
        two_fingers(&mut h, 6000);
        assert!(h.due(8000), "a new touch fires again");
    }
    #[test]
    fn one_finger_three_fingers_or_a_lifted_one_never_fire() {
        let mut h = armed();
        finger(&mut h, 0, 7, Some((300, 400)));
        sync(&mut h, 1000);
        assert!(!h.due(9000), "a stock long press is one finger");
        finger(&mut h, 1, 8, Some((600, 450)));
        sync(&mut h, 9000);
        finger(&mut h, 2, 9, Some((500, 900)));
        sync(&mut h, 9100);
        finger(&mut h, 2, -1, None);
        sync(&mut h, 9200);
        assert!(!h.due(20000), "a third finger spends the touch");
        let mut h = armed();
        two_fingers(&mut h, 1000);
        finger(&mut h, 1, -1, None);
        sync(&mut h, 2500);
        assert!(!h.due(4000));
        finger(&mut h, 1, 9, Some((610, 450)));
        sync(&mut h, 4000);
        assert!(
            !h.due(5999) && h.due(6000),
            "putting the finger back starts over"
        );
    }
    #[test]
    fn stale_presses_moves_and_dropped_events_wait_for_a_release() {
        // Fingers already down when the observer opened.
        let mut h = Hold {
            span: (1000, 1000),
            ..Hold::default()
        };
        two_fingers(&mut h, 1000);
        assert!(!h.due(9000));
        lift(&mut h, 9100);
        two_fingers(&mut h, 10000);
        assert!(h.due(12000));
        // A swipe: more than a twelfth of the axis.
        let mut h = armed();
        two_fingers(&mut h, 1000);
        h.event(3, ABS_MT_SLOT, 0, 0);
        h.event(3, ABS_MT_POSITION_X, 390, 0);
        sync(&mut h, 1500);
        assert!(!h.due(4000));
        // A small tremble is still a hold.
        let mut h = armed();
        two_fingers(&mut h, 1000);
        h.event(3, ABS_MT_SLOT, 0, 0);
        h.event(3, ABS_MT_POSITION_X, 340, 0);
        sync(&mut h, 1500);
        assert!(h.due(3000));
        // Dropped events: the device is read again, and fingers still down wait for a release.
        let mut h = armed();
        two_fingers(&mut h, 1000);
        h.event(0, 3, 0, 1100);
        h.event(3, ABS_MT_POSITION_X, 1, 1100);
        assert!(!h.stale, "events up to the next report are discarded");
        sync(&mut h, 1200);
        assert!(std::mem::take(&mut h.stale));
        let mut ids = [-1; SLOTS];
        ids[..2].copy_from_slice(&[7, 8]);
        h.resync(1, ids);
        assert!(!h.due(5000));
        h.event(3, ABS_MT_POSITION_Y, 460, 5100);
        sync(&mut h, 5100);
        assert!(!h.due(9000), "fingers from before the gap do not count");
        lift(&mut h, 9100);
        two_fingers(&mut h, 10000);
        assert!(h.due(12000));
    }
    #[test]
    fn fingers_found_down_at_open_count_once_every_one_lifts() {
        let mut ids = [-1; SLOTS];
        ids[3] = 41;
        let mut h = Hold {
            span: (1000, 1000),
            ..Hold::default()
        };
        h.resync(3, ids);
        // A finger the stream never introduced moves, and a new one joins it.
        h.event(3, ABS_MT_POSITION_X, 500, 0);
        sync(&mut h, 100);
        finger(&mut h, 0, 7, Some((300, 400)));
        sync(&mut h, 1000);
        assert_eq!(h.fingers(), 2);
        assert!(!h.due(9000));
        finger(&mut h, 0, -1, None);
        sync(&mut h, 9100);
        finger(&mut h, 3, -1, None);
        sync(&mut h, 9200);
        two_fingers(&mut h, 10000);
        assert!(h.due(12000));
        // Nothing down at open: the first touch counts.
        let mut h = Hold {
            span: (1000, 1000),
            ..Hold::default()
        };
        h.resync(0, [-1; SLOTS]);
        two_fingers(&mut h, 1000);
        assert!(h.due(3000));
    }
    #[test]
    fn holding_off_spends_the_touch_in_progress() {
        let mut h = armed();
        two_fingers(&mut h, 1000);
        h.hold_off();
        assert!(!h.due(9000));
        lift(&mut h, 9100);
        two_fingers(&mut h, 10000);
        assert!(h.due(12000));
    }
}
