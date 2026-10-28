//! IMAP modified UTF-7 mailbox names.
//! Source: RFC 3501 §5.1.3 — "&" starts a modified-base64 run of UTF-16BE, "-" ends it, "&-" is a literal "&",
//! modified base64 uses "," instead of "/" and has no "=" padding; printable US-ASCII 0x20–0x7e stands for itself.
use base64::alphabet::Alphabet;
use base64::engine::{general_purpose::GeneralPurposeConfig, DecodePaddingMode, GeneralPurpose};
use base64::Engine as _;
use std::sync::OnceLock;

fn engine() -> &'static GeneralPurpose {
    static E: OnceLock<GeneralPurpose> = OnceLock::new();
    E.get_or_init(|| {
        let alpha = Alphabet::new("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+,").expect("valid alphabet");
        GeneralPurpose::new(
            &alpha,
            GeneralPurposeConfig::new().with_encode_padding(false).with_decode_padding_mode(DecodePaddingMode::Indifferent),
        )
    })
}

pub fn decode(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 1..];
        let Some(end) = after.find('-') else {
            out.push_str(&rest[pos..]);
            return out;
        };
        let chunk = &after[..end];
        if chunk.is_empty() {
            out.push('&');
        } else if let Ok(bytes) = engine().decode(chunk) {
            let units: Vec<u16> = bytes.chunks(2).filter(|c| c.len() == 2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
            out.push_str(&String::from_utf16_lossy(&units));
        } else {
            out.push('&');
            out.push_str(chunk);
            out.push('-');
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

pub fn encode(s: &str) -> String {
    let mut out = String::new();
    let mut pending: Vec<u16> = Vec::new();
    let flush = |pending: &mut Vec<u16>, out: &mut String| {
        if !pending.is_empty() {
            let bytes: Vec<u8> = pending.iter().flat_map(|u| u.to_be_bytes()).collect();
            out.push('&');
            out.push_str(&engine().encode(bytes));
            out.push('-');
            pending.clear();
        }
    };
    for ch in s.chars() {
        if (' '..='~').contains(&ch) {
            flush(&mut pending, &mut out);
            if ch == '&' {
                out.push_str("&-");
            } else {
                out.push(ch);
            }
        } else {
            let mut buf = [0u16; 2];
            pending.extend_from_slice(ch.encode_utf16(&mut buf));
        }
    }
    flush(&mut pending, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        // Source: RFC 3501 §5.1.3 example "~peter/mail/&U,BTFw-/&ZeVnLIqe-" = "~peter/mail/台北/日本語".
        assert_eq!(decode("~peter/mail/&U,BTFw-/&ZeVnLIqe-"), "~peter/mail/台北/日本語");
        assert_eq!(encode("~peter/mail/台北/日本語"), "~peter/mail/&U,BTFw-/&ZeVnLIqe-");
        assert_eq!(decode("Tom &- Jerry"), "Tom & Jerry");
        assert_eq!(encode("Tom & Jerry"), "Tom &- Jerry");
        assert_eq!(decode(&encode("Entwürfe")), "Entwürfe");
        assert_eq!(decode("INBOX"), "INBOX");
    }
}
