//! Bounded NIP-44 version 2 encryption.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use chacha20::{
    ChaCha20,
    cipher::{KeyIvInit, StreamCipher},
};
use hmac::{Hmac, KeyInit, Mac};
use k256::{
    ecdh::diffie_hellman,
    schnorr::{SigningKey, VerifyingKey},
};
use sha2::Sha256;
use zeroize::Zeroize;

use crate::{EnvelopeError, FailureClass, MAX_NIP44_PAYLOAD_BYTES, MAX_PLAINTEXT_BYTES};

const VERSION: u8 = 2;
const MIN_ENCODED_BYTES: usize = 132;
const MIN_DECODED_BYTES: usize = 99;

pub(crate) fn conversation_key(
    secret: &SigningKey,
    peer: [u8; 32],
) -> Result<[u8; 32], EnvelopeError> {
    let public = VerifyingKey::from_slice(&peer)
        .map_err(|_| EnvelopeError::new(FailureClass::InvalidPublicKey))?;
    let shared = diffie_hellman(secret.as_nonzero_scalar(), public.as_affine());
    let mut extract = <Hmac<Sha256> as KeyInit>::new_from_slice(b"nip44-v2")
        .map_err(|_| EnvelopeError::new(FailureClass::Cryptography))?;
    extract.update(shared.raw_secret_bytes());
    Ok(extract.finalize().into_bytes().into())
}

pub(crate) fn encrypt(
    plaintext: &[u8],
    conversation_key: &[u8; 32],
    nonce: [u8; 32],
) -> Result<String, EnvelopeError> {
    let mut padded = pad(plaintext)?;
    let mut keys = expand_keys(conversation_key, &nonce)?;
    let mut cipher = ChaCha20::new_from_slices(&keys[..32], &keys[32..44])
        .map_err(|_| EnvelopeError::new(FailureClass::Cryptography))?;
    cipher.apply_keystream(&mut padded);
    let mac = authenticate(&keys[44..], &nonce, &padded)?;
    let mut payload = Vec::with_capacity(65 + padded.len());
    payload.push(VERSION);
    payload.extend_from_slice(&nonce);
    payload.extend_from_slice(&padded);
    payload.extend_from_slice(&mac);
    let encoded = STANDARD.encode(payload);
    padded.zeroize();
    keys.zeroize();
    if encoded.len() > MAX_NIP44_PAYLOAD_BYTES {
        return Err(EnvelopeError::new(FailureClass::Size));
    }
    Ok(encoded)
}

pub(crate) fn decrypt(
    payload: &str,
    conversation_key: &[u8; 32],
) -> Result<Vec<u8>, EnvelopeError> {
    if payload.starts_with('#') {
        return Err(EnvelopeError::new(FailureClass::UnsupportedEncryption));
    }
    if !(MIN_ENCODED_BYTES..=MAX_NIP44_PAYLOAD_BYTES).contains(&payload.len()) {
        return Err(EnvelopeError::new(FailureClass::Size));
    }
    let mut decoded = STANDARD
        .decode(payload)
        .map_err(|_| EnvelopeError::new(FailureClass::MalformedEncryption))?;
    if decoded.len() < MIN_DECODED_BYTES || decoded[0] != VERSION {
        let class = if decoded.len() < MIN_DECODED_BYTES {
            FailureClass::MalformedEncryption
        } else {
            FailureClass::UnsupportedEncryption
        };
        decoded.zeroize();
        return Err(EnvelopeError::new(class));
    }
    let ciphertext_end = decoded.len() - 32;
    let nonce: [u8; 32] = decoded[1..33]
        .try_into()
        .map_err(|_| EnvelopeError::new(FailureClass::MalformedEncryption))?;
    let mut keys = expand_keys(conversation_key, &nonce)?;
    let wanted = authenticate(&keys[44..], &nonce, &decoded[33..ciphertext_end])?;
    if !constant_time_equal(&wanted, &decoded[ciphertext_end..]) {
        keys.zeroize();
        decoded.zeroize();
        return Err(EnvelopeError::new(FailureClass::Mac));
    }
    let mut padded = decoded[33..ciphertext_end].to_vec();
    decoded.zeroize();
    let mut cipher = ChaCha20::new_from_slices(&keys[..32], &keys[32..44])
        .map_err(|_| EnvelopeError::new(FailureClass::Cryptography))?;
    cipher.apply_keystream(&mut padded);
    keys.zeroize();
    let plaintext = unpad(&padded)?;
    padded.zeroize();
    Ok(plaintext)
}

fn expand_keys(key: &[u8; 32], nonce: &[u8; 32]) -> Result<[u8; 76], EnvelopeError> {
    let mut output = [0_u8; 76];
    let mut previous = Vec::new();
    let mut offset = 0;
    for counter in 1_u8..=3 {
        let mut hmac = <Hmac<Sha256> as KeyInit>::new_from_slice(key)
            .map_err(|_| EnvelopeError::new(FailureClass::Cryptography))?;
        hmac.update(&previous);
        hmac.update(nonce);
        hmac.update(&[counter]);
        previous = hmac.finalize().into_bytes().to_vec();
        let count = (output.len() - offset).min(previous.len());
        output[offset..offset + count].copy_from_slice(&previous[..count]);
        offset += count;
    }
    previous.zeroize();
    Ok(output)
}

fn authenticate(
    key: &[u8],
    nonce: &[u8; 32],
    ciphertext: &[u8],
) -> Result<[u8; 32], EnvelopeError> {
    let mut hmac = <Hmac<Sha256> as KeyInit>::new_from_slice(key)
        .map_err(|_| EnvelopeError::new(FailureClass::Cryptography))?;
    hmac.update(nonce);
    hmac.update(ciphertext);
    Ok(hmac.finalize().into_bytes().into())
}

fn pad(plaintext: &[u8]) -> Result<Vec<u8>, EnvelopeError> {
    if plaintext.is_empty() || plaintext.len() > MAX_PLAINTEXT_BYTES {
        return Err(EnvelopeError::new(FailureClass::Size));
    }
    let extended = plaintext.len() >= 65_536;
    let prefix_len = if extended { 6 } else { 2 };
    let padded_len = padded_len(plaintext.len())?;
    let mut output = vec![0; prefix_len + padded_len];
    if extended {
        let length =
            u32::try_from(plaintext.len()).map_err(|_| EnvelopeError::new(FailureClass::Size))?;
        output[2..6].copy_from_slice(&length.to_be_bytes());
    } else {
        let length =
            u16::try_from(plaintext.len()).map_err(|_| EnvelopeError::new(FailureClass::Size))?;
        output[..2].copy_from_slice(&length.to_be_bytes());
    }
    output[prefix_len..prefix_len + plaintext.len()].copy_from_slice(plaintext);
    Ok(output)
}

fn unpad(padded: &[u8]) -> Result<Vec<u8>, EnvelopeError> {
    if padded.len() < 34 {
        return Err(EnvelopeError::new(FailureClass::Padding));
    }
    let short = u16::from_be_bytes([padded[0], padded[1]]);
    let (length, prefix_len) = if short == 0 {
        if padded.len() < 6 {
            return Err(EnvelopeError::new(FailureClass::Padding));
        }
        let length = u32::from_be_bytes([padded[2], padded[3], padded[4], padded[5]]) as usize;
        if length < 65_536 {
            return Err(EnvelopeError::new(FailureClass::Padding));
        }
        (length, 6)
    } else {
        (usize::from(short), 2)
    };
    if length == 0
        || length > MAX_PLAINTEXT_BYTES
        || padded_len(length)
            .ok()
            .and_then(|value| value.checked_add(prefix_len))
            != Some(padded.len())
    {
        return Err(EnvelopeError::new(FailureClass::Padding));
    }
    let end = prefix_len
        .checked_add(length)
        .ok_or_else(|| EnvelopeError::new(FailureClass::Padding))?;
    let plaintext = padded
        .get(prefix_len..end)
        .ok_or_else(|| EnvelopeError::new(FailureClass::Padding))?;
    if padded[end..].iter().any(|byte| *byte != 0) {
        return Err(EnvelopeError::new(FailureClass::Padding));
    }
    std::str::from_utf8(plaintext).map_err(|_| EnvelopeError::new(FailureClass::Padding))?;
    Ok(plaintext.to_vec())
}

fn padded_len(length: usize) -> Result<usize, EnvelopeError> {
    if length <= 32 {
        return Ok(32);
    }
    let next_power = length
        .checked_next_power_of_two()
        .ok_or_else(|| EnvelopeError::new(FailureClass::Size))?;
    let chunk = if next_power <= 256 {
        32
    } else {
        next_power / 8
    };
    length
        .checked_add(chunk - 1)
        .map(|value| value / chunk * chunk)
        .ok_or_else(|| EnvelopeError::new(FailureClass::Size))
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret(value: u8) -> SigningKey {
        let mut bytes = [0; 32];
        bytes[31] = value;
        SigningKey::from_slice(&bytes).unwrap_or_else(|_| unreachable!())
    }

    #[test]
    fn matches_published_nip44_vector() {
        let one = secret(1);
        let two = secret(2);
        let key = conversation_key(&one, two.verifying_key().to_bytes().into())
            .unwrap_or_else(|_| unreachable!());
        assert_eq!(
            hex(&key),
            "c41c775356fd92eadc63ff5a0dc1da211b268cbea22316767095b2871ea1412d"
        );
        let mut nonce = [0; 32];
        nonce[31] = 1;
        let payload = encrypt(b"a", &key, nonce).unwrap_or_else(|_| unreachable!());
        assert_eq!(
            payload,
            "AgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABee0G5VSK0/9YypIObAtDKfYEAjD35uVkHyB0F4DwrcNaCXlCWZKaArsGrY6M9wnuTMxWfp1RTN9Xga8no+kF5Vsb"
        );
        let swapped = conversation_key(&two, one.verifying_key().to_bytes().into())
            .unwrap_or_else(|_| unreachable!());
        assert_eq!(swapped, key);
        assert_eq!(
            decrypt(&payload, &swapped).unwrap_or_else(|_| unreachable!()),
            b"a"
        );
    }

    #[test]
    fn supports_extended_lengths_and_rejects_tampering() {
        let key = [7; 32];
        let plaintext = vec![b'x'; 65_536];
        let payload = encrypt(&plaintext, &key, [9; 32]).unwrap_or_else(|_| unreachable!());
        assert_eq!(
            decrypt(&payload, &key).unwrap_or_else(|_| unreachable!()),
            plaintext
        );
        let mut decoded = STANDARD.decode(payload).unwrap_or_else(|_| unreachable!());
        let last = decoded.len() - 1;
        decoded[last] ^= 1;
        assert_eq!(
            decrypt(&STANDARD.encode(decoded), &key).map_err(EnvelopeError::class),
            Err(FailureClass::Mac)
        );
    }

    fn hex(bytes: &[u8]) -> String {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            output.push(char::from(DIGITS[usize::from(byte >> 4)]));
            output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
        }
        output
    }
}
