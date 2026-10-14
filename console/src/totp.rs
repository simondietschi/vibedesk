//! Einmalcodes nach RFC 6238 (TOTP, SHA-1, 6 Stellen, 30 Sekunden), wie sie
//! Authenticator-Apps erzeugen.

use data_encoding::BASE32_NOPAD;
use hmac::{Hmac, Mac};
use qrcode::{render::svg, QrCode};
use rand::RngCore;
use sha1::Sha1;

const STEP_SECS: i64 = 30;
const DIGITS: u32 = 6;

/// Neues Geheimnis mit 160 Bit, Base32-codiert.
pub fn generate_secret() -> String {
    let mut bytes = [0u8; 20];
    rand::thread_rng().fill_bytes(&mut bytes);
    BASE32_NOPAD.encode(&bytes)
}

fn code_at(secret: &[u8], step: i64) -> Option<u32> {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret).ok()?;
    mac.update(&step.to_be_bytes());
    let hash = mac.finalize().into_bytes();
    let offset = (hash[hash.len() - 1] & 0x0f) as usize;
    let value = u32::from_be_bytes([
        hash[offset] & 0x7f,
        hash[offset + 1],
        hash[offset + 2],
        hash[offset + 3],
    ]);
    Some(value % 10u32.pow(DIGITS))
}

/// Prüft einen Code mit einer Toleranz von einem Zeitschritt in beide
/// Richtungen. Gibt den Zeitschritt des Treffers zurück, damit derselbe Code
/// nicht zweimal verwendet werden kann.
pub fn verify(secret_b32: &str, code: &str, now: i64) -> Option<i64> {
    let code = code.trim().replace(' ', "");
    if code.len() != DIGITS as usize || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let code: u32 = code.parse().ok()?;
    let secret = BASE32_NOPAD.decode(secret_b32.as_bytes()).ok()?;
    let current = now / STEP_SECS;
    (current - 1..=current + 1).find(|&step| code_at(&secret, step) == Some(code))
}

/// Aktueller Code, wie ihn eine Authenticator-App anzeigen würde.
pub fn code(secret_b32: &str, now: i64) -> Option<String> {
    let secret = BASE32_NOPAD.decode(secret_b32.as_bytes()).ok()?;
    code_at(&secret, now / STEP_SECS).map(|c| format!("{c:06}"))
}

pub fn otpauth_url(secret_b32: &str, username: &str, issuer: &str) -> String {
    format!(
        "otpauth://totp/{issuer}:{user}?secret={secret_b32}&issuer={issuer}&algorithm=SHA1&digits={DIGITS}&period={STEP_SECS}",
        issuer = percent_encode(issuer),
        user = percent_encode(username),
    )
}

pub fn qr_svg(data: &str) -> anyhow::Result<String> {
    let code = QrCode::new(data.as_bytes())?;
    Ok(code
        .render::<svg::Color>()
        .min_dimensions(200, 200)
        .quiet_zone(true)
        .build())
}

fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Testvektoren aus RFC 6238, Anhang B (SHA-1, auf 6 Stellen gekürzt).
    #[test]
    fn rfc6238_vectors() {
        let secret = b"12345678901234567890";
        assert_eq!(code_at(secret, 59 / 30), Some(287082));
        assert_eq!(code_at(secret, 1111111109 / 30), Some(81804));
        assert_eq!(code_at(secret, 2000000000 / 30), Some(279037));
    }

    #[test]
    fn verify_accepts_neighbouring_steps_only() {
        let secret = generate_secret();
        let raw = BASE32_NOPAD.decode(secret.as_bytes()).unwrap();
        let now = 1_700_000_000;
        let code = format!("{:06}", code_at(&raw, now / 30 - 1).unwrap());
        assert_eq!(verify(&secret, &code, now), Some(now / 30 - 1));
        assert_eq!(verify(&secret, &code, now + 90), None);
        assert_eq!(verify(&secret, "12345", now), None);
    }

    #[test]
    fn otpauth_url_escapes_names() {
        let url = otpauth_url("ABC", "anna müller", "VibeDesk");
        assert!(url.starts_with("otpauth://totp/VibeDesk:anna%20m%C3%BCller?secret=ABC&"));
    }
}
