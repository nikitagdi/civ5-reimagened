//! The shared half of the example mod `hello`: the messages on channel 0 and the saved-state format.
//!
//! Plain data with a hand-written codec, so the mod needs no serialization crate. Both halves of a mod depend on
//! this crate; it depends on nothing.
//!
//! Wire format (little-endian):
//!
//! | Message | Direction | Bytes |
//! |---|---|---|
//! | `Say` | client to server | `1`, `len u16`, UTF-8 text |
//! | `Echo` | server to client | `2`, `from i8`, `count u32`, `len u16`, UTF-8 text |
//! | `State` | server to client | `3`, `count u32` |

#![forbid(unsafe_code)]

use std::fmt;

/// The mod id, as in `mod.toml`.
pub const MOD_ID: &str = "hello";
/// The echo channel (`[[channels]] id = 0`).
pub const CH_ECHO: u8 = 0;
/// Longest text of one `Say`, in bytes.
pub const MAX_TEXT_BYTES: usize = 256;

const TAG_SAY: u8 = 1;
const TAG_ECHO: u8 = 2;
const TAG_STATE: u8 = 3;

/// A message of the echo channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    /// A client says something.
    Say { text: String },
    /// The server repeats it to everybody, with the speaker's player id and the running count.
    Echo { from: i8, count: u32, text: String },
    /// The count so far, sent to a client that attaches.
    State { count: u32 },
}

/// Why bytes are not a [`Msg`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    Empty,
    UnknownTag(u8),
    Truncated,
    TrailingBytes(usize),
    TextTooLong(usize),
    NotUtf8,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Empty => write!(f, "empty message"),
            DecodeError::UnknownTag(t) => write!(f, "unknown message tag {t}"),
            DecodeError::Truncated => write!(f, "message is truncated"),
            DecodeError::TrailingBytes(n) => write!(f, "{n} unexpected bytes after the message"),
            DecodeError::TextTooLong(n) => {
                write!(
                    f,
                    "text of {n} bytes exceeds the {MAX_TEXT_BYTES} byte limit"
                )
            }
            DecodeError::NotUtf8 => write!(f, "text is not UTF-8"),
        }
    }
}

impl std::error::Error for DecodeError {}

impl Msg {
    /// The message as bytes. Text over [`MAX_TEXT_BYTES`] is cut at a character boundary.
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Msg::Say { text } => {
                let t = clip(text);
                let mut out = vec![TAG_SAY];
                put_text(&mut out, t);
                out
            }
            Msg::Echo { from, count, text } => {
                let mut out = vec![TAG_ECHO, *from as u8];
                out.extend_from_slice(&count.to_le_bytes());
                put_text(&mut out, clip(text));
                out
            }
            Msg::State { count } => {
                let mut out = vec![TAG_STATE];
                out.extend_from_slice(&count.to_le_bytes());
                out
            }
        }
    }

    /// Strict decode: unknown tags, short input, trailing bytes, over-long or non-UTF-8 text are errors.
    pub fn decode(b: &[u8]) -> Result<Msg, DecodeError> {
        let (&tag, rest) = b.split_first().ok_or(DecodeError::Empty)?;
        match tag {
            TAG_SAY => {
                let (text, rest) = take_text(rest)?;
                done(rest, Msg::Say { text })
            }
            TAG_ECHO => {
                let (&from, rest) = rest.split_first().ok_or(DecodeError::Truncated)?;
                let (count, rest) = take_u32(rest)?;
                let (text, rest) = take_text(rest)?;
                done(
                    rest,
                    Msg::Echo {
                        from: from as i8,
                        count,
                        text,
                    },
                )
            }
            TAG_STATE => {
                let (count, rest) = take_u32(rest)?;
                done(rest, Msg::State { count })
            }
            t => Err(DecodeError::UnknownTag(t)),
        }
    }
}

fn done(rest: &[u8], m: Msg) -> Result<Msg, DecodeError> {
    if rest.is_empty() {
        Ok(m)
    } else {
        Err(DecodeError::TrailingBytes(rest.len()))
    }
}

fn clip(s: &str) -> &str {
    if s.len() <= MAX_TEXT_BYTES {
        return s;
    }
    let mut end = MAX_TEXT_BYTES;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn put_text(out: &mut Vec<u8>, t: &str) {
    out.extend_from_slice(&(t.len() as u16).to_le_bytes());
    out.extend_from_slice(t.as_bytes());
}

fn take_u32(b: &[u8]) -> Result<(u32, &[u8]), DecodeError> {
    let head: [u8; 4] = b
        .get(..4)
        .ok_or(DecodeError::Truncated)?
        .try_into()
        .map_err(|_| DecodeError::Truncated)?;
    Ok((u32::from_le_bytes(head), &b[4..]))
}

fn take_text(b: &[u8]) -> Result<(String, &[u8]), DecodeError> {
    let head: [u8; 2] = b
        .get(..2)
        .ok_or(DecodeError::Truncated)?
        .try_into()
        .map_err(|_| DecodeError::Truncated)?;
    let len = usize::from(u16::from_le_bytes(head));
    if len > MAX_TEXT_BYTES {
        return Err(DecodeError::TextTooLong(len));
    }
    let body = b.get(2..2 + len).ok_or(DecodeError::Truncated)?;
    let text = std::str::from_utf8(body).map_err(|_| DecodeError::NotUtf8)?;
    Ok((text.to_string(), &b[2 + len..]))
}

/// Current version of the saved state.
pub const STATE_VERSION: u8 = 1;
/// Size of the saved state in bytes.
pub const STATE_LEN: usize = 5;

/// The state for the save: `version u8`, `count u32`.
pub fn encode_state(count: u32) -> Vec<u8> {
    let mut out = vec![STATE_VERSION];
    out.extend_from_slice(&count.to_le_bytes());
    out
}

/// The count from saved state; `None` for an unknown version or a wrong length (the mod then starts fresh).
pub fn decode_state(b: &[u8]) -> Option<u32> {
    match b {
        [STATE_VERSION, a, b2, c, d] => Some(u32::from_le_bytes([*a, *b2, *c, *d])),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt(m: Msg) {
        assert_eq!(Msg::decode(&m.encode()).unwrap(), m);
    }

    #[test]
    fn round_trips() {
        rt(Msg::Say {
            text: String::new(),
        });
        rt(Msg::Say {
            text: "hello мир".into(),
        });
        rt(Msg::Echo {
            from: -1,
            count: u32::MAX,
            text: "x".repeat(MAX_TEXT_BYTES),
        });
        rt(Msg::Echo {
            from: 7,
            count: 0,
            text: "a".into(),
        });
        rt(Msg::State { count: 12345 });
    }

    #[test]
    fn golden_bytes() {
        assert_eq!(
            Msg::Say { text: "hi".into() }.encode(),
            [1, 2, 0, b'h', b'i']
        );
        assert_eq!(
            Msg::Echo {
                from: 3,
                count: 258,
                text: "a".into()
            }
            .encode(),
            [2, 3, 2, 1, 0, 0, 1, 0, b'a']
        );
        assert_eq!(Msg::State { count: 1 }.encode(), [3, 1, 0, 0, 0]);
    }

    #[test]
    fn strict_decode() {
        assert_eq!(Msg::decode(&[]), Err(DecodeError::Empty));
        assert_eq!(Msg::decode(&[9]), Err(DecodeError::UnknownTag(9)));
        assert_eq!(Msg::decode(&[1, 5, 0, b'a']), Err(DecodeError::Truncated));
        assert_eq!(Msg::decode(&[1]), Err(DecodeError::Truncated));
        assert_eq!(Msg::decode(&[3, 1, 0]), Err(DecodeError::Truncated));
        assert_eq!(
            Msg::decode(&[3, 1, 0, 0, 0, 9]),
            Err(DecodeError::TrailingBytes(1))
        );
        assert_eq!(
            Msg::decode(&[1, 2, 0, 0xff, 0xfe]),
            Err(DecodeError::NotUtf8)
        );
        let mut long = vec![1];
        long.extend_from_slice(&((MAX_TEXT_BYTES + 1) as u16).to_le_bytes());
        long.extend(std::iter::repeat_n(b'a', MAX_TEXT_BYTES + 1));
        assert_eq!(
            Msg::decode(&long),
            Err(DecodeError::TextTooLong(MAX_TEXT_BYTES + 1))
        );
    }

    #[test]
    fn encode_clips_long_text_on_a_character_boundary() {
        let text = "é".repeat(MAX_TEXT_BYTES); // 2 bytes each
        let Msg::Say { text: back } = Msg::decode(&Msg::Say { text }.encode()).unwrap() else {
            panic!("not a Say")
        };
        assert_eq!(back.len(), MAX_TEXT_BYTES);
        assert!(back.chars().all(|c| c == 'é'));
    }

    #[test]
    fn state_format() {
        assert_eq!(encode_state(7), [1, 7, 0, 0, 0]);
        assert_eq!(encode_state(7).len(), STATE_LEN);
        assert_eq!(decode_state(&encode_state(0xdead_beef)), Some(0xdead_beef));
        assert_eq!(decode_state(&[2, 0, 0, 0, 0]), None);
        assert_eq!(decode_state(&[1, 0, 0]), None);
        assert_eq!(decode_state(&[]), None);
    }
}
