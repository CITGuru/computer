//! A view-only filter in front of a VNC server that has no view-only mode.

use crate::error::{Error, Result};

/// Client-to-server messages, by their leading byte.
mod message {
    pub const SET_PIXEL_FORMAT: u8 = 0;
    pub const SET_ENCODINGS: u8 = 2;
    pub const FRAMEBUFFER_UPDATE_REQUEST: u8 = 3;
    pub const KEY_EVENT: u8 = 4;
    pub const POINTER_EVENT: u8 = 5;
    pub const CLIENT_CUT_TEXT: u8 = 6;
}

/// Security types this filter can follow to the end of the handshake.
mod security {
    pub const NONE: u8 = 1;
    pub const VNC_AUTH: u8 = 2;
}

/// The `DesktopSize` pseudo-encoding.
pub const DESKTOP_SIZE: i32 = -223;

/// The reply to a VNC-auth challenge: DES over the sixteen-byte challenge.
const AUTH_RESPONSE: usize = 16;

/// `RFB 003.008\n` and its siblings.
const VERSION: usize = 12;

/// What to do with the bytes at the front of the client's stream.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    /// Consume this many and pass them on unchanged.
    Forward(usize),
    /// Consume this many and pass on nothing.
    Drop(usize),
    /// Consume this many and pass on these instead.
    Replace(usize, Vec<u8>),
}

impl Step {
    fn taken(&self) -> usize {
        match self {
            Self::Forward(taken) | Self::Drop(taken) | Self::Replace(taken, _) => *taken,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The client's twelve-byte version reply.
    Version,
    /// Under 3.3 the client chooses nothing: the server names one type and the
    /// client answers it. Held here until the server has said which, because
    /// that is what decides whether a challenge answer is coming.
    ServerChoice,
    /// The one byte naming the security type it chose. 3.7 and later only.
    Security,
    /// Its answer to the challenge, where the type asks for one.
    AuthResponse,
    /// The shared-desktop flag.
    ClientInit,
    /// Everything after, which is where input lives.
    Messages,
}

/// A client-to-server RFB stream with the input taken out of it.
#[derive(Debug)]
pub struct ViewOnly {
    phase: Phase,
    held: Vec<u8>,
    /// How much of the server's own handshake has gone past.
    server_seen: usize,
    /// What the server chose, under 3.3.
    server_choice: Option<u8>,
}

impl Default for ViewOnly {
    fn default() -> Self {
        Self::new()
    }
}

impl ViewOnly {
    pub fn new() -> Self {
        Self {
            phase: Phase::Version,
            held: Vec::new(),
            server_seen: 0,
            server_choice: None,
        }
    }

    /// Watch the server's half of the handshake.
    pub fn server_said(&mut self, bytes: &[u8]) {
        for byte in bytes {
            // The server's version line, then — under 3.3 only — the four
            // bytes naming its choice. After that this stops looking.
            match self.server_seen {
                0..=11 => {}
                12..=15 => {
                    // Big-endian u32; only the last byte can name a type.
                    if self.server_seen == 15 {
                        self.server_choice = Some(*byte);
                        if self.phase == Phase::ServerChoice {
                            self.phase = match *byte {
                                security::VNC_AUTH => Phase::AuthResponse,
                                _ => Phase::ClientInit,
                            };
                        }
                    }
                }
                _ => return,
            }
            self.server_seen += 1;
        }
    }

    /// Whether the handshake is done and messages are being read.
    pub fn watching(&self) -> bool {
        self.phase == Phase::Messages
    }

    /// What may be forwarded, given these bytes from the client.
    pub fn filter(&mut self, bytes: &[u8]) -> Result<Vec<u8>> {
        // Taken out of `self` so the phase can advance while the buffer is
        // still being read, and so nothing is copied per message.
        let mut held = std::mem::take(&mut self.held);
        held.extend_from_slice(bytes);

        let mut forward = Vec::new();
        let mut at = 0;

        let outcome = loop {
            match self.next(&held[at..]) {
                Ok(Some(step)) => {
                    let taken = step.taken();
                    match step {
                        Step::Forward(_) => forward.extend_from_slice(&held[at..at + taken]),
                        Step::Drop(_) => {}
                        Step::Replace(_, bytes) => forward.extend_from_slice(&bytes),
                    }
                    at += taken;
                }
                Ok(None) => break Ok(forward),
                Err(error) => break Err(error),
            }
        };

        held.drain(..at);
        self.held = held;
        outcome
    }

    /// One step: how many bytes this consumes, and whether they go on.
    fn next(&mut self, buffer: &[u8]) -> Result<Option<Step>> {
        match self.phase {
            Phase::Version => {
                if buffer.len() < VERSION {
                    return Ok(None);
                }
                // Only 3.7 and later send a security type the client picks. In
                // 3.3 the server chooses and the client says nothing, so this
                // filter cannot tell whether a challenge answer is coming and
                // would lose framing on the first message.
                let version = std::str::from_utf8(&buffer[..VERSION]).unwrap_or_default();

                self.phase = if version.starts_with("RFB 003.003") {
                    // macOS Screen Sharing answers 3.3 whatever the server
                    // offered, so this is the common case rather than a legacy
                    // one. The type comes from the server; see `server_said`.
                    match self.server_choice {
                        Some(security::VNC_AUTH) => Phase::AuthResponse,
                        Some(_) => Phase::ClientInit,
                        None => Phase::ServerChoice,
                    }
                } else if version.starts_with("RFB 003.007") || version.starts_with("RFB 003.008") {
                    Phase::Security
                } else {
                    return Err(Error::denied(format!(
                        "a view-only viewer cannot follow {:?}",
                        version.trim_end()
                    )));
                };

                Ok(Some(Step::Forward(VERSION)))
            }

            // Nothing may be forwarded until the server has said which
            // security type it picked: the next sixteen bytes are either an
            // answer to a challenge or the start of the messages.
            Phase::ServerChoice => Ok(None),

            Phase::Security => {
                let Some(chosen) = buffer.first().copied() else {
                    return Ok(None);
                };
                self.phase = match chosen {
                    security::NONE => Phase::ClientInit,
                    security::VNC_AUTH => Phase::AuthResponse,
                    other => {
                        return Err(Error::denied(format!(
                            "security type {other} has a handshake this filter \
                             cannot follow, and a lost frame is a control port"
                        )));
                    }
                };
                Ok(Some(Step::Forward(1)))
            }

            Phase::AuthResponse => {
                if buffer.len() < AUTH_RESPONSE {
                    return Ok(None);
                }
                self.phase = Phase::ClientInit;
                Ok(Some(Step::Forward(AUTH_RESPONSE)))
            }

            Phase::ClientInit => {
                if buffer.is_empty() {
                    return Ok(None);
                }
                self.phase = Phase::Messages;
                Ok(Some(Step::Forward(1)))
            }

            Phase::Messages => Ok(message_length(buffer)?.map(|(length, kind)| {
                if changes_the_screen(kind) {
                    return Step::Drop(length);
                }
                // The one message edited rather than judged: see
                // [`DESKTOP_SIZE`].
                match with_desktop_size(&buffer[..length], kind) {
                    Some(rewritten) => Step::Replace(length, rewritten),
                    None => Step::Forward(length),
                }
            })),
        }
    }
}

/// Whether a message does anything other than ask to be shown pixels.
fn changes_the_screen(kind: u8) -> bool {
    matches!(
        kind,
        // The clipboard is in here because a paste into the guest is input,
        // even though nothing moves on the way in.
        message::KEY_EVENT | message::POINTER_EVENT | message::CLIENT_CUT_TEXT
    )
}

/// How long the message at the front of `buffer` is, and what kind it is.
fn message_length(buffer: &[u8]) -> Result<Option<(usize, u8)>> {
    let Some(kind) = buffer.first().copied() else {
        return Ok(None);
    };

    let length = match kind {
        message::SET_PIXEL_FORMAT => 20,
        message::FRAMEBUFFER_UPDATE_REQUEST => 10,
        message::KEY_EVENT => 8,
        message::POINTER_EVENT => 6,

        message::SET_ENCODINGS => {
            let Some(count) = be_u16(buffer, 2) else {
                return Ok(None);
            };
            4 + 4 * count as usize
        }

        message::CLIENT_CUT_TEXT => {
            let Some(length) = be_u32(buffer, 4) else {
                return Ok(None);
            };
            // Held whole before it is dropped: the body has to be counted out
            // of the stream or the next byte read is text, not a message type.
            8 + length as usize
        }

        other => {
            return Err(Error::denied(format!(
                "client message {other} is one this filter does not know the \
                 length of, and a wrong length is a lost frame"
            )));
        }
    };

    Ok((buffer.len() >= length).then_some((length, kind)))
}

/// `SetEncodings` with [`DESKTOP_SIZE`] added, where it was not already there.
fn with_desktop_size(message: &[u8], kind: u8) -> Option<Vec<u8>> {
    if kind != message::SET_ENCODINGS {
        return None;
    }

    let count = be_u16(message, 2)? as usize;
    let body = message.get(4..4 + count * 4)?;

    let already = body.chunks_exact(4).any(|encoding| {
        encoding
            .try_into()
            .map(|bytes| i32::from_be_bytes(bytes) == DESKTOP_SIZE)
            .unwrap_or(false)
    });

    // A full list cannot be added to: a count that wrapped would describe a
    // shorter message than the bytes behind it, which is a lost frame.
    if already || count >= u16::MAX as usize {
        return None;
    }

    let mut rewritten = vec![message[0], message[1]];
    rewritten.extend_from_slice(&((count + 1) as u16).to_be_bytes());
    rewritten.extend_from_slice(body);
    rewritten.extend_from_slice(&DESKTOP_SIZE.to_be_bytes());
    Some(rewritten)
}

fn be_u16(buffer: &[u8], at: usize) -> Option<u16> {
    let bytes = buffer.get(at..at + 2)?;
    Some(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn be_u32(buffer: &[u8], at: usize) -> Option<u32> {
    let bytes = buffer.get(at..at + 4)?;
    Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Through the handshake, so the tests that care about messages start
    /// where messages start.
    fn watching() -> ViewOnly {
        let mut filter = ViewOnly::new();
        filter.filter(b"RFB 003.008\n").expect("a version");
        filter.filter(&[security::NONE]).expect("a security type");
        filter.filter(&[1]).expect("client init");
        assert!(filter.watching());
        filter
    }

    fn pointer(x: u16, y: u16) -> Vec<u8> {
        let mut event = vec![message::POINTER_EVENT, 1];
        event.extend_from_slice(&x.to_be_bytes());
        event.extend_from_slice(&y.to_be_bytes());
        event
    }

    fn key(down: u8, keysym: u32) -> Vec<u8> {
        let mut event = vec![message::KEY_EVENT, down, 0, 0];
        event.extend_from_slice(&keysym.to_be_bytes());
        event
    }

    fn update_request() -> Vec<u8> {
        vec![
            message::FRAMEBUFFER_UPDATE_REQUEST,
            1,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ]
    }

    #[test]
    fn test_the_handshake_goes_through_untouched() {
        let mut filter = ViewOnly::new();

        assert_eq!(filter.filter(b"RFB 003.008\n").unwrap(), b"RFB 003.008\n");
        assert_eq!(filter.filter(&[security::NONE]).unwrap(), vec![1]);
        assert_eq!(filter.filter(&[1]).unwrap(), vec![1]);
        assert!(filter.watching());
    }

    #[test]
    fn test_an_auth_reply_is_counted_out_before_messages_begin() {
        let mut filter = ViewOnly::new();
        filter.filter(b"RFB 003.008\n").unwrap();
        filter.filter(&[security::VNC_AUTH]).unwrap();

        assert!(!filter.watching(), "the challenge answer has not arrived");
        filter.filter(&[0; AUTH_RESPONSE]).unwrap();
        filter.filter(&[1]).unwrap();

        assert!(
            filter.watching(),
            "sixteen bytes read as messages would be a pointer event nobody sent"
        );
    }

    #[test]
    fn test_a_pointer_event_never_reaches_the_server() {
        assert!(
            watching().filter(&pointer(10, 20)).unwrap().is_empty(),
            "this is the whole point of the port"
        );
    }

    #[test]
    fn test_a_key_event_never_reaches_the_server() {
        assert!(watching().filter(&key(1, 0x0061)).unwrap().is_empty());
    }

    #[test]
    fn test_a_paste_into_the_guest_is_input_too() {
        let mut cut = vec![message::CLIENT_CUT_TEXT, 0, 0, 0];
        cut.extend_from_slice(&5u32.to_be_bytes());
        cut.extend_from_slice(b"hello");

        assert!(
            watching().filter(&cut).unwrap().is_empty(),
            "nothing moves on the way in, but the guest's clipboard changed"
        );
    }

    #[test]
    fn test_asking_to_be_shown_pixels_is_allowed() {
        let request = update_request();
        assert_eq!(watching().filter(&request).unwrap(), request);
    }

    #[test]
    fn test_the_stream_keeps_its_framing_around_a_dropped_message() {
        let mut filter = watching();
        let mut stream = pointer(1, 2);
        stream.extend_from_slice(&update_request());
        stream.extend_from_slice(&key(1, 0x0061));
        stream.extend_from_slice(&update_request());

        assert_eq!(
            filter.filter(&stream).unwrap(),
            [update_request(), update_request()].concat(),
            "a dropped message still has to be counted out, or the next byte \
             read is the middle of this one"
        );
    }

    #[test]
    fn test_a_message_split_across_reads_is_held_until_it_is_whole() {
        let mut filter = watching();
        let request = update_request();

        assert!(
            filter.filter(&request[..4]).unwrap().is_empty(),
            "half a message forwarded is a message the server misreads"
        );
        assert_eq!(filter.filter(&request[4..]).unwrap(), request);
    }

    #[test]
    fn test_a_clipboard_body_is_counted_even_though_it_is_dropped() {
        let mut filter = watching();
        let mut stream = vec![message::CLIENT_CUT_TEXT, 0, 0, 0];
        stream.extend_from_slice(&4u32.to_be_bytes());
        // Text that is itself a valid update request, so a filter that failed
        // to count the body would forward it as one.
        stream.extend_from_slice(&[message::FRAMEBUFFER_UPDATE_REQUEST, 1, 0, 0]);
        stream.extend_from_slice(&update_request());

        assert_eq!(
            filter.filter(&stream).unwrap(),
            update_request(),
            "the body has to be counted out of the stream, not scanned"
        );
    }

    #[test]
    fn test_a_variable_encoding_list_is_measured_from_its_count() {
        let mut filter = watching();
        let mut encodings = vec![message::SET_ENCODINGS, 0];
        encodings.extend_from_slice(&2u16.to_be_bytes());
        encodings.extend_from_slice(&0i32.to_be_bytes());
        encodings.extend_from_slice(&1i32.to_be_bytes());

        let sent = filter.filter(&encodings).unwrap();

        // Measured, then rewritten: the list goes out one longer than it came
        // in. See `DESKTOP_SIZE`.
        assert_eq!(u16::from_be_bytes([sent[2], sent[3]]), 3);
        assert_eq!(sent.len(), encodings.len() + 4);
        assert!(sent.ends_with(&DESKTOP_SIZE.to_be_bytes()));
    }

    #[test]
    fn test_a_protocol_nobody_can_follow_is_refused_rather_than_guessed_at() {
        let mut filter = ViewOnly::new();

        assert!(
            filter.filter(b"RFB 004.001\n").is_err(),
            "a version whose handshake is a different shape puts every later \
             message one field out, and a misread message is a control port"
        );
    }

    #[test]
    fn test_three_three_is_followed_rather_than_refused() {
        let mut filter = ViewOnly::new();
        filter.server_said(b"RFB 003.008\n");

        assert!(
            filter.filter(b"RFB 003.003\n").is_ok(),
            "macOS Screen Sharing answers 3.3 whatever the server offered, so \
             refusing it refuses the client every Mac ships"
        );
    }

    #[test]
    fn test_an_unknown_security_type_ends_the_connection() {
        let mut filter = ViewOnly::new();
        filter.filter(b"RFB 003.008\n").unwrap();

        assert!(
            filter.filter(&[19]).is_err(),
            "a lost frame is a control port"
        );
    }

    #[test]
    fn test_an_unknown_message_ends_the_connection() {
        assert!(
            watching().filter(&[200, 0, 0, 0]).is_err(),
            "a message of unknown length cannot be counted out, and a wrong \
             length forwards the middle of it as a new message"
        );
    }
}

#[cfg(test)]
mod version_three_three {
    use super::*;

    /// The exact shape macOS Screen Sharing negotiates, as captured from a
    /// live connection: it answers 3.3 whatever the server offered, and the
    /// server then names one security type in four bytes.
    fn apple_handshake() -> ViewOnly {
        let mut filter = ViewOnly::new();

        filter.server_said(b"RFB 003.008\n");
        assert_eq!(filter.filter(b"RFB 003.003\n").unwrap(), b"RFB 003.003\n");
        assert!(
            !filter.watching(),
            "the type has not been announced, so nothing can be framed yet"
        );

        filter.server_said(&[0, 0, 0, security::VNC_AUTH]);
        filter.filter(&[0xAB; AUTH_RESPONSE]).expect("the answer");
        filter.filter(&[1]).expect("client init");
        filter
    }

    #[test]
    fn test_apple_screen_sharing_reaches_the_message_phase() {
        assert!(
            apple_handshake().watching(),
            "rejecting 3.3 is rejecting the client every Mac ships"
        );
    }

    #[test]
    fn test_input_is_still_dropped_once_it_gets_there() {
        let mut filter = apple_handshake();
        let request = vec![
            message::FRAMEBUFFER_UPDATE_REQUEST,
            1,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ];

        assert!(
            filter
                .filter(&[message::POINTER_EVENT, 1, 0, 10, 0, 20])
                .unwrap()
                .is_empty()
        );
        assert_eq!(filter.filter(&request).unwrap(), request);
    }

    #[test]
    fn test_a_server_that_asks_for_nothing_needs_no_answer() {
        let mut filter = ViewOnly::new();
        filter.server_said(b"RFB 003.008\n");
        filter.filter(b"RFB 003.003\n").unwrap();
        filter.server_said(&[0, 0, 0, security::NONE]);

        filter.filter(&[1]).expect("client init");
        assert!(
            filter.watching(),
            "waiting for sixteen bytes that are never sent reads the first \
             message as a challenge answer"
        );
    }

    #[test]
    fn test_the_messages_apple_actually_sends_are_all_understood() {
        let mut filter = apple_handshake();

        // Captured from a live Screen Sharing session: SetEncodings with
        // thirteen encodings, SetPixelFormat, then update requests.
        let mut encodings = vec![message::SET_ENCODINGS, 0];
        encodings.extend_from_slice(&13u16.to_be_bytes());
        encodings.extend_from_slice(&[0u8; 52]);
        assert_eq!(
            filter.filter(&encodings).unwrap().len(),
            60,
            "fifty-six in, sixty out: the viewer adds the pseudo-encoding the \
             server aborts without"
        );

        let pixel_format = vec![0u8; 20];
        assert_eq!(filter.filter(&pixel_format).unwrap().len(), 20);
    }
}

#[cfg(test)]
mod advertising {
    use super::*;

    fn watching() -> ViewOnly {
        let mut filter = ViewOnly::new();
        filter.server_said(b"RFB 003.008\n");
        filter.filter(b"RFB 003.003\n").expect("a version");
        filter.server_said(&[0, 0, 0, security::NONE]);
        filter.filter(&[1]).expect("client init");
        filter
    }

    fn encodings(list: &[i32]) -> Vec<u8> {
        let mut message = vec![message::SET_ENCODINGS, 0];
        message.extend_from_slice(&(list.len() as u16).to_be_bytes());
        for encoding in list {
            message.extend_from_slice(&encoding.to_be_bytes());
        }
        message
    }

    fn listed(message: &[u8]) -> Vec<i32> {
        let count = u16::from_be_bytes([message[2], message[3]]) as usize;
        message[4..4 + count * 4]
            .chunks_exact(4)
            .map(|e| i32::from_be_bytes(e.try_into().expect("four bytes")))
            .collect()
    }

    #[test]
    fn test_a_client_that_omits_it_is_given_it_anyway() {
        let sent = watching().filter(&encodings(&[0])).expect("encodings");

        assert_eq!(
            listed(&sent),
            vec![0, DESKTOP_SIZE],
            "the server aborts the whole process on a client without it, so \
             the viewer supplies it rather than letting a client choose to \
             take the guest down"
        );
    }

    #[test]
    fn test_the_count_matches_the_list_it_describes() {
        let sent = watching()
            .filter(&encodings(&[0, 1, -239]))
            .expect("encodings");

        assert_eq!(u16::from_be_bytes([sent[2], sent[3]]) as usize, 4);
        assert_eq!(
            sent.len(),
            4 + 4 * 4,
            "a count that disagrees with the body puts every later message one \
             field out"
        );
    }

    #[test]
    fn test_a_client_that_already_asked_is_left_alone() {
        let asked = encodings(&[0, DESKTOP_SIZE, 1]);
        let sent = watching().filter(&asked).expect("encodings");

        assert_eq!(
            sent, asked,
            "adding it twice is a list that describes itself wrongly"
        );
    }

    #[test]
    fn test_nothing_else_is_rewritten() {
        let mut filter = watching();
        let request = vec![
            message::FRAMEBUFFER_UPDATE_REQUEST,
            1,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ];

        assert_eq!(filter.filter(&request).expect("a request"), request);
        assert!(
            filter
                .filter(&[message::POINTER_EVENT, 1, 0, 10, 0, 20])
                .expect("a pointer event")
                .is_empty(),
            "input is still dropped, and rewriting must not have made a way in"
        );
    }
}
