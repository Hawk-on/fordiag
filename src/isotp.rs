//! Reassembly of ISO-TP (ISO 15765-2) messages from ELM327 output.
//!
//! With headers on (`ATH1`) and spaces on (`ATS1`), the ELM327 prints every CAN
//! frame it receives as one line: the 11-bit CAN ID followed by the data bytes,
//! including the ISO-TP PCI byte. For example:
//!
//! ```text
//! 7E8 06 41 00 BE 3F A8 13          single frame, 6 bytes
//! 72E 10 0F 59 02 FF F0 03 16       first frame, 15 bytes in total
//! 72E 21 08 D2 00 00 2C 91 17       consecutive frame 1
//! 72E 22 62 28 AA AA AA AA AA       consecutive frame 2 (padding at the end)
//! ```
//!
//! The ELM327 sends the flow control frames itself (with `ATCAF1`), so we only
//! need to put the received frames back together into messages.

use std::collections::HashMap;
use thiserror::Error;

/// One reassembled message from one CAN ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub id: u16,
    pub data: Vec<u8>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum IsoTpError {
    #[error("could not parse frame line \"{0}\"")]
    BadLine(String),
    #[error("invalid ISO-TP frame from {id:03X}: {reason}")]
    BadFrame { id: u16, reason: &'static str },
    #[error("consecutive frame from {id:03X} without a first frame")]
    UnexpectedConsecutive { id: u16 },
    #[error("wrong sequence number from {id:03X}: expected {expected}, got {got}")]
    Sequence { id: u16, expected: u8, got: u8 },
    #[error("incomplete message from {id:03X}: got {got} of {expected} bytes")]
    Incomplete { id: u16, expected: usize, got: usize },
}

/// One CAN frame as printed by the ELM327.
struct Frame {
    id: u16,
    data: Vec<u8>,
}

/// Parse a line such as `7E8 06 41 00 BE 3F A8 13`.
fn parse_line(line: &str) -> Option<Frame> {
    let mut tokens = line.split_whitespace();
    let id_text = tokens.next()?;
    if id_text.len() != 3 {
        return None;
    }
    let id = u16::from_str_radix(id_text, 16).ok()?;
    let data = tokens
        .map(|t| {
            if t.len() == 2 {
                u8::from_str_radix(t, 16).ok()
            } else {
                None
            }
        })
        .collect::<Option<Vec<u8>>>()?;
    if data.is_empty() {
        return None;
    }
    Some(Frame { id, data })
}

/// A multi-frame message that is still being received.
struct Partial {
    expected_len: usize,
    data: Vec<u8>,
    next_seq: u8,
}

/// Put the frames in `lines` back together into complete messages, in the order
/// they were completed.
pub fn reassemble(lines: &[String]) -> Result<Vec<Message>, IsoTpError> {
    let mut done = Vec::new();
    let mut partial: HashMap<u16, Partial> = HashMap::new();

    for line in lines {
        let frame = parse_line(line).ok_or_else(|| IsoTpError::BadLine(line.clone()))?;
        let id = frame.id;
        let d = &frame.data;
        let bad = |reason| IsoTpError::BadFrame { id, reason };

        match d[0] >> 4 {
            // Single frame: low nibble is the length.
            0x0 => {
                let len = (d[0] & 0x0F) as usize;
                if len == 0 || len > d.len() - 1 {
                    return Err(bad("single frame length does not fit"));
                }
                done.push(Message {
                    id,
                    data: d[1..=len].to_vec(),
                });
            }
            // First frame: 12-bit length, then the first data bytes.
            0x1 => {
                if d.len() < 2 {
                    return Err(bad("first frame too short"));
                }
                let expected_len = (((d[0] & 0x0F) as usize) << 8) | d[1] as usize;
                if expected_len < 8 {
                    return Err(bad("first frame length below 8"));
                }
                partial.insert(
                    id,
                    Partial {
                        expected_len,
                        data: d[2..].to_vec(),
                        next_seq: 1,
                    },
                );
            }
            // Consecutive frame: low nibble is the sequence number (wraps at 15).
            0x2 => {
                let seq = d[0] & 0x0F;
                let p = partial.get_mut(&id).ok_or(IsoTpError::UnexpectedConsecutive { id })?;
                if seq != p.next_seq {
                    return Err(IsoTpError::Sequence {
                        id,
                        expected: p.next_seq,
                        got: seq,
                    });
                }
                p.data.extend_from_slice(&d[1..]);
                p.next_seq = (p.next_seq + 1) & 0x0F;
                if p.data.len() >= p.expected_len {
                    let mut p = partial.remove(&id).unwrap();
                    // Drop padding bytes after the real end of the message.
                    p.data.truncate(p.expected_len);
                    done.push(Message { id, data: p.data });
                }
            }
            // Flow control frames are not part of the payload.
            0x3 => {}
            _ => return Err(bad("unknown PCI type")),
        }
    }

    if let Some((&id, p)) = partial.iter().next() {
        return Err(IsoTpError::Incomplete {
            id,
            expected: p.expected_len,
            got: p.data.len(),
        });
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn single_frame() {
        let m = reassemble(&lines(&["7E8 06 41 00 BE 3F A8 13"])).unwrap();
        assert_eq!(
            m,
            vec![Message {
                id: 0x7E8,
                data: vec![0x41, 0x00, 0xBE, 0x3F, 0xA8, 0x13]
            }]
        );
    }

    #[test]
    fn single_frame_with_padding() {
        let m = reassemble(&lines(&["7E8 02 43 00 00 00 00 00"])).unwrap();
        assert_eq!(m[0].data, vec![0x43, 0x00]);
    }

    #[test]
    fn multi_frame_with_padding() {
        let m = reassemble(&lines(&[
            "72E 10 0F 59 02 FF F0 03 16",
            "72E 21 08 D2 00 00 2C 91 17",
            "72E 22 62 28 AA AA AA AA AA",
        ]))
        .unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].id, 0x72E);
        assert_eq!(
            m[0].data,
            vec![
                0x59, 0x02, 0xFF, 0xF0, 0x03, 0x16, 0x08, 0xD2, 0x00, 0x00, 0x2C, 0x91, 0x17, 0x62, 0x28
            ]
        );
    }

    #[test]
    fn several_messages_from_same_id() {
        // A "response pending" followed by the real answer.
        let m = reassemble(&lines(&["72E 03 7F 19 78", "72E 03 59 02 FF"])).unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(m[1].data, vec![0x59, 0x02, 0xFF]);
    }

    #[test]
    fn errors() {
        assert!(matches!(reassemble(&lines(&["NO DATA"])), Err(IsoTpError::BadLine(_))));
        assert!(matches!(
            reassemble(&lines(&["72E 21 00 00 00 00 00 00 00"])),
            Err(IsoTpError::UnexpectedConsecutive { .. })
        ));
        assert!(matches!(
            reassemble(&lines(&["72E 10 0F 59 02 FF F0 03 16", "72E 22 00 00 00 00 00 00 00"])),
            Err(IsoTpError::Sequence { .. })
        ));
        assert!(matches!(
            reassemble(&lines(&["72E 10 0F 59 02 FF F0 03 16"])),
            Err(IsoTpError::Incomplete { .. })
        ));
        assert!(matches!(
            reassemble(&lines(&["7E8 07 41 00"])),
            Err(IsoTpError::BadFrame { .. })
        ));
    }
}
