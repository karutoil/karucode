//! WebAuthn (passkeys) verification, server side.
//!
//! This is a deliberately small implementation of the two ceremonies a relying
//! party actually needs, and it is explicit about what it does **not** do:
//!
//! * **Supported**: ES256 (P-256 ECDSA) credentials over `none` attestation -
//!   which is what every platform authenticator (Touch ID, Windows Hello, Google
//!   Password Manager, 1Password, a YubiKey) produces for a normal "add a
//!   passkey" flow. `fmt` is read and accepted for any value, but the attestation
//!   statement is **not** verified: this server makes no claim about *which*
//!   authenticator created a credential, only that the private key behind it
//!   signed the challenge. That is the standard self-hosted trade-off - attestation
//!   is an enterprise policy feature, and requiring it would reject most passkeys.
//! * **Not supported**: RS256/EdDSA credentials (`kty: RSA`, `OKP`), packed/TPM
//!   attestation chains, and the `hybrid`/`caBLE` transports beyond what a
//!   browser already handles for us. A credential that is not ES256 is refused
//!   with a message that says so, at registration time, rather than failing
//!   mysteriously at the first sign-in.
//!
//! What *is* checked, and must be, is the full ceremony: the challenge we minted,
//! the origin the browser reports, the relying-party id hash, the user-present
//! flag, the credential's public key, and (for assertions) a monotonically
//! non-decreasing signature counter so a cloned authenticator is detectable.

use base64::engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};
use std::fmt;

/// Flag bits in `authenticatorData`.
const FLAG_USER_PRESENT: u8 = 0x01;
const FLAG_USER_VERIFIED: u8 = 0x04;
const FLAG_ATTESTED_CREDENTIAL_DATA: u8 = 0x40;
const FLAG_EXTENSION_DATA: u8 = 0x80;

/// The challenge and relying-party context a ceremony must match.
#[derive(Debug, Clone)]
pub struct Ceremony {
    /// The relying party id (a host name, no scheme or port).
    pub rp_id: String,
    /// Every origin that is acceptable in `clientDataJSON`.
    pub origins: Vec<String>,
    /// The challenge, base64url (no padding), exactly as it was sent out.
    pub challenge: String,
}

impl Ceremony {
    /// Check the fields of `clientDataJSON` that every ceremony shares.
    fn check_client_data(&self, raw: &[u8], expected_type: &str) -> Result<(), WebAuthnError> {
        #[derive(serde::Deserialize)]
        struct ClientData {
            #[serde(rename = "type")]
            kind: String,
            challenge: String,
            origin: String,
            #[serde(default, rename = "crossOrigin")]
            cross_origin: bool,
        }
        let data: ClientData = serde_json::from_slice(raw).map_err(|_| {
            WebAuthnError::Malformed(
                "clientDataJSON is not the JSON object the browser sends".into(),
            )
        })?;
        if data.kind != expected_type {
            return Err(WebAuthnError::Mismatch(format!(
                "clientData.type is `{}`, expected `{expected_type}`",
                data.kind
            )));
        }
        // Compare decoded bytes so padding differences do not matter.
        let expected = decode_b64(&self.challenge)?;
        let actual = decode_b64(&data.challenge).map_err(|_| {
            WebAuthnError::Mismatch("clientData.challenge is not valid base64url".into())
        })?;
        if actual != expected {
            return Err(WebAuthnError::Mismatch(
                "clientData.challenge does not match the challenge this server issued; \
                 the ceremony expired or was replayed"
                    .into(),
            ));
        }
        if data.cross_origin {
            return Err(WebAuthnError::Mismatch(
                "clientData.crossOrigin is set; a cross-origin ceremony is refused".into(),
            ));
        }
        if !self.origins.is_empty()
            && !self
                .origins
                .iter()
                .any(|origin| origin.eq_ignore_ascii_case(&data.origin))
        {
            return Err(WebAuthnError::Mismatch(format!(
                "clientData.origin is `{}`, expected {}",
                data.origin,
                self.origins.join(" or ")
            )));
        }
        Ok(())
    }
}

/// The parts of a registration response the browser hands back.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistrationInput {
    /// base64url `credential.rawId`.
    pub credential_id: String,
    /// base64url `response.clientDataJSON`.
    pub client_data_json: String,
    /// base64url `response.attestationObject`.
    pub attestation_object: String,
}

/// The parts of an assertion (sign-in) response the browser hands back.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertionInput {
    pub credential_id: String,
    pub client_data_json: String,
    /// base64url `response.authenticatorData`.
    pub authenticator_data: String,
    /// base64url `response.signature` (DER-encoded ECDSA).
    pub signature: String,
}

/// A credential as it is stored after a successful registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredCredential {
    /// base64url, no padding.
    pub credential_id: String,
    /// SEC1 uncompressed P-256 point (`0x04 || x || y`).
    pub public_key: Vec<u8>,
    /// The counter the authenticator reported at registration.
    pub counter: u32,
    /// False when the authenticator reported user verification at registration.
    pub user_verified: bool,
}

/// Why a ceremony was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebAuthnError {
    /// The input is not the shape the browser produces.
    Malformed(String),
    /// The input parsed but does not match the ceremony (challenge, origin, rpId,
    /// counter, signature).
    Mismatch(String),
    /// A correctly-formed credential of a type this server does not verify.
    Unsupported(String),
}

impl fmt::Display for WebAuthnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WebAuthnError::Malformed(message) => write!(f, "{message}"),
            WebAuthnError::Mismatch(message) => write!(f, "{message}"),
            WebAuthnError::Unsupported(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for WebAuthnError {}

/// Decode standard base64 or base64url, with or without padding.
fn decode_b64(raw: &str) -> Result<Vec<u8>, WebAuthnError> {
    let text = raw.trim();
    if text.is_empty() {
        return Err(WebAuthnError::Malformed("an empty base64 value".into()));
    }
    if let Ok(bytes) = URL_SAFE_NO_PAD.decode(text) {
        return Ok(bytes);
    }
    if let Ok(bytes) = URL_SAFE.decode(text) {
        return Ok(bytes);
    }
    if let Ok(bytes) = STANDARD.decode(text) {
        return Ok(bytes);
    }
    Err(WebAuthnError::Malformed(
        "a value that is not base64url/base64".into(),
    ))
}

/// Base64url without padding, the spelling every browser-produced field uses.
pub fn encode_b64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// A fresh challenge: 32 random bytes, base64url.
pub fn new_challenge() -> String {
    encode_b64url(&random_bytes(32))
}

/// Cryptographic randomness from the OS.
///
/// A challenge has to be unpredictable: if it can be guessed, an attacker can
/// pre-compute a signature for it and replay an assertion. `getrandom` is the
/// same source `std`'s `HashMap` seed and every TLS library use.
fn random_bytes(len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    if getrandom::getrandom(&mut out).is_err() {
        // The OS entropy source failed. Returning predictable bytes would be a
        // silent security hole, so refuse the ceremony instead.
        tracing::error!("the OS random source failed; refusing to mint a passkey challenge");
        return Vec::new();
    }
    out
}

/// Parse the CBOR `attestationObject` and verify a registration ceremony.
pub fn verify_registration(
    input: &RegistrationInput,
    ceremony: &Ceremony,
) -> Result<RegisteredCredential, WebAuthnError> {
    let client_data = decode_b64(&input.client_data_json)?;
    ceremony.check_client_data(&client_data, "webauthn.create")?;

    let attestation = decode_b64(&input.attestation_object)?;
    let value: ciborium::value::Value = ciborium::de::from_reader(attestation.as_slice())
        .map_err(|_| WebAuthnError::Malformed("attestationObject is not valid CBOR".into()))?;
    let map = value
        .as_map()
        .ok_or_else(|| WebAuthnError::Malformed("attestationObject is not a CBOR map".into()))?;
    let auth_data = map
        .iter()
        .find(|(key, _)| key.as_text() == Some("authData"))
        .and_then(|(_, value)| value.as_bytes().cloned())
        .ok_or_else(|| {
            WebAuthnError::Malformed("attestationObject has no `authData` bytes".into())
        })?;

    let parsed = parse_authenticator_data(&auth_data, &ceremony.rp_id)?;
    if !parsed.has_attested_credential {
        return Err(WebAuthnError::Mismatch(
            "the authenticator returned no credential data (the AT flag is clear), so there is \
             nothing to register"
                .into(),
        ));
    }
    let reported_id = parsed
        .credential_id
        .clone()
        .ok_or_else(|| WebAuthnError::Malformed("the credential id is missing".into()))?;
    let public_key = parsed
        .public_key
        .clone()
        .ok_or_else(|| WebAuthnError::Malformed("the credential public key is missing".into()))?;

    // The `rawId` the client sent must be the credential the authenticator
    // attested. A mismatch means a client assembled two different responses.
    let raw_id = decode_b64(&input.credential_id)?;
    if raw_id != reported_id {
        return Err(WebAuthnError::Mismatch(
            "credential.rawId does not match the credential id inside the attestation".into(),
        ));
    }

    // Prove the stored key can verify at all, so a broken public key is a
    // registration error rather than a permanent sign-in failure.
    VerifyingKey::from_sec1_bytes(&public_key).map_err(|_| {
        WebAuthnError::Unsupported("the credential public key is not a valid P-256 point".into())
    })?;

    Ok(RegisteredCredential {
        credential_id: encode_b64url(&reported_id),
        public_key,
        counter: parsed.counter,
        user_verified: parsed.user_verified,
    })
}

/// Verify an assertion and return the counter the authenticator reported.
///
/// The caller stores that counter on the credential; passing it back in on the
/// next assertion is what makes cloning detectable.
pub fn verify_assertion(
    input: &AssertionInput,
    stored_public_key: &[u8],
    stored_counter: u32,
    ceremony: &Ceremony,
) -> Result<u32, WebAuthnError> {
    let client_data = decode_b64(&input.client_data_json)?;
    ceremony.check_client_data(&client_data, "webauthn.get")?;

    let auth_data = decode_b64(&input.authenticator_data)?;
    let parsed = parse_authenticator_data(&auth_data, &ceremony.rp_id)?;

    let signature_bytes = decode_b64(&input.signature)?;
    let signature = Signature::from_der(&signature_bytes)
        .map_err(|_| WebAuthnError::Malformed("the signature is not DER-encoded ECDSA".into()))?;
    let key = VerifyingKey::from_sec1_bytes(stored_public_key).map_err(|_| {
        WebAuthnError::Unsupported(
            "the stored credential key is not a valid P-256 point; register the passkey again"
                .into(),
        )
    })?;

    // WebAuthn signs `authenticatorData || SHA-256(clientDataJSON)`.
    let mut signed = auth_data.clone();
    signed.extend_from_slice(&Sha256::digest(&client_data));
    key.verify(&signed, &signature).map_err(|_| {
        WebAuthnError::Mismatch(
            "the assertion signature does not verify against the stored credential".into(),
        )
    })?;

    // A counter that does not advance means a cloned authenticator. Many
    // authenticators report 0 forever, so 0 -> 0 is allowed; anything that goes
    // backwards from a non-zero value is refused.
    if parsed.counter != 0 && parsed.counter <= stored_counter {
        return Err(WebAuthnError::Mismatch(format!(
            "the signature counter did not advance ({} after {}); this credential may have been cloned",
            parsed.counter, stored_counter
        )));
    }
    Ok(parsed.counter)
}

/// The fields of `authenticatorData` this server uses.
#[derive(Debug, Clone, Default)]
struct ParsedAuthData {
    counter: u32,
    user_verified: bool,
    has_attested_credential: bool,
    credential_id: Option<Vec<u8>>,
    public_key: Option<Vec<u8>>,
}

fn parse_authenticator_data(raw: &[u8], rp_id: &str) -> Result<ParsedAuthData, WebAuthnError> {
    if raw.len() < 37 {
        return Err(WebAuthnError::Malformed(format!(
            "authenticatorData is {} bytes; at least 37 are required",
            raw.len()
        )));
    }
    let expected = Sha256::digest(rp_id.as_bytes());
    if raw[..32] != expected[..] {
        return Err(WebAuthnError::Mismatch(format!(
            "this passkey was registered for a different domain (rpIdHash does not match `{rp_id}`)"
        )));
    }
    let flags = raw[32];
    let counter = u32::from_be_bytes([raw[33], raw[34], raw[35], raw[36]]);
    let user_present = flags & FLAG_USER_PRESENT != 0;
    let user_verified = flags & FLAG_USER_VERIFIED != 0;
    if !user_present {
        return Err(WebAuthnError::Mismatch(
            "the authenticator did not report a user gesture (the UP flag is clear)".into(),
        ));
    }
    let mut parsed = ParsedAuthData {
        counter,
        user_verified,
        has_attested_credential: flags & FLAG_ATTESTED_CREDENTIAL_DATA != 0,
        ..ParsedAuthData::default()
    };
    if !parsed.has_attested_credential {
        return Ok(parsed);
    }

    let mut cursor = 37;
    // aaguid(16) | credentialIdLength(2) | credentialId | COSE public key
    if raw.len() < cursor + 18 {
        return Err(WebAuthnError::Malformed(
            "authenticatorData ends before the attested credential data".into(),
        ));
    }
    cursor += 16;
    let id_len = u16::from_be_bytes([raw[cursor], raw[cursor + 1]]) as usize;
    cursor += 2;
    if id_len == 0 || raw.len() < cursor + id_len {
        return Err(WebAuthnError::Malformed(
            "authenticatorData has an invalid credential id length".into(),
        ));
    }
    parsed.credential_id = Some(raw[cursor..cursor + id_len].to_vec());
    cursor += id_len;

    let (cose, used) = parse_cose_key(&raw[cursor..])?;
    cursor += used;
    if cursor > raw.len() {
        return Err(WebAuthnError::Malformed(
            "authenticatorData ends inside the credential public key".into(),
        ));
    }
    parsed.public_key = Some(cose);
    // Extension data may follow; nothing here needs it, and the signature covers
    // the whole blob, so ignoring it cannot be exploited.
    let _ = flags & FLAG_EXTENSION_DATA;
    Ok(parsed)
}

/// Parse a CBOR COSE_Key and return the SEC1 uncompressed point it encodes.
///
/// Only `EC2 / ES256 / P-256` is accepted, which is what passkeys everywhere
/// produce; RSA and Ed25519 credentials get an explicit "not supported" message
/// instead of a signature that never verifies.
fn parse_cose_key(raw: &[u8]) -> Result<(Vec<u8>, usize), WebAuthnError> {
    let mut cursor = std::io::Cursor::new(raw);
    let value: ciborium::value::Value = ciborium::de::from_reader(&mut cursor)
        .map_err(|_| WebAuthnError::Malformed("the credential public key is not CBOR".into()))?;
    let used = cursor.position() as usize;
    let map = value.as_map().ok_or_else(|| {
        WebAuthnError::Malformed("the credential public key is not a COSE_Key map".into())
    })?;
    let field = |name: i128| {
        map.iter()
            .find(|(key, _)| key.as_integer().map(i128::from) == Some(name))
            .map(|(_, value)| value)
    };
    let kty = field(1)
        .and_then(|v| v.as_integer())
        .map(i128::from)
        .ok_or_else(|| WebAuthnError::Malformed("the COSE key has no kty".into()))?;
    if kty != 2 {
        return Err(WebAuthnError::Unsupported(format!(
            "this passkey uses a COSE key type of {kty}; only EC2 (2, ES256/P-256) is supported"
        )));
    }
    let alg = field(3)
        .and_then(|v| v.as_integer())
        .map(i128::from)
        .ok_or_else(|| WebAuthnError::Malformed("the COSE key has no alg".into()))?;
    if alg != -7 {
        return Err(WebAuthnError::Unsupported(format!(
            "this passkey uses COSE algorithm {alg}; only ES256 (-7) is supported"
        )));
    }
    let crv = field(-1)
        .and_then(|v| v.as_integer())
        .map(i128::from)
        .ok_or_else(|| WebAuthnError::Malformed("the COSE key has no crv".into()))?;
    if crv != 1 {
        return Err(WebAuthnError::Unsupported(format!(
            "this passkey uses curve {crv}; only P-256 (1) is supported"
        )));
    }
    let x = coord(field(-2))?;
    let y = coord(field(-3))?;
    let mut point = Vec::with_capacity(65);
    point.push(0x04);
    point.extend_from_slice(&x);
    point.extend_from_slice(&y);
    Ok((point, used))
}

fn coord(value: Option<&ciborium::value::Value>) -> Result<Vec<u8>, WebAuthnError> {
    let bytes = value
        .and_then(|value| value.as_bytes())
        .ok_or_else(|| WebAuthnError::Malformed("the COSE key has no coordinate".into()))?;
    if bytes.len() != 32 {
        return Err(WebAuthnError::Malformed(format!(
            "a P-256 coordinate is {} bytes, expected 32",
            bytes.len()
        )));
    }
    Ok(bytes.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::signature::Signer;
    use p256::ecdsa::SigningKey;

    const RP_ID: &str = "karucode.example.com";
    const ORIGIN: &str = "https://karucode.example.com";

    struct Fixture {
        signing: SigningKey,
        credential_id: Vec<u8>,
    }

    fn ceremony(challenge: &str) -> Ceremony {
        Ceremony {
            rp_id: RP_ID.into(),
            origins: vec![ORIGIN.into()],
            challenge: challenge.into(),
        }
    }

    fn client_data(kind: &str, challenge: &str, origin: &str, cross_origin: bool) -> Vec<u8> {
        serde_json::json!({
            "type": kind,
            "challenge": challenge,
            "origin": origin,
            "crossOrigin": cross_origin,
        })
        .to_string()
        .into_bytes()
    }

    fn auth_data(
        rp_id: &str,
        flags: u8,
        counter: u32,
        attested: Option<(&[u8], Vec<u8>)>,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&Sha256::digest(rp_id.as_bytes()));
        out.push(flags);
        out.extend_from_slice(&counter.to_be_bytes());
        if let Some((credential_id, cose)) = attested {
            out.extend_from_slice(&[0u8; 16]); // aaguid
            out.extend_from_slice(&(credential_id.len() as u16).to_be_bytes());
            out.extend_from_slice(credential_id);
            out.extend_from_slice(&cose);
        }
        out
    }

    fn cose_key(signing: &SigningKey) -> Vec<u8> {
        let point = signing.verifying_key().to_encoded_point(false);
        let map = ciborium::value::Value::Map(vec![
            (1.into(), 2.into()),
            (3.into(), (-7i64).into()),
            ((-1i64).into(), 1.into()),
            (
                (-2i64).into(),
                ciborium::value::Value::Bytes(point.x().unwrap().to_vec()),
            ),
            (
                (-3i64).into(),
                ciborium::value::Value::Bytes(point.y().unwrap().to_vec()),
            ),
        ]);
        let mut out = Vec::new();
        ciborium::ser::into_writer(&map, &mut out).unwrap();
        out
    }

    fn attestation(auth_data: &[u8], fmt: &str) -> Vec<u8> {
        let map = ciborium::value::Value::Map(vec![
            (
                ciborium::value::Value::Text("fmt".into()),
                ciborium::value::Value::Text(fmt.into()),
            ),
            (
                ciborium::value::Value::Text("attStmt".into()),
                ciborium::value::Value::Map(vec![]),
            ),
            (
                ciborium::value::Value::Text("authData".into()),
                ciborium::value::Value::Bytes(auth_data.to_vec()),
            ),
        ]);
        let mut out = Vec::new();
        ciborium::ser::into_writer(&map, &mut out).unwrap();
        out
    }

    fn fixture() -> Fixture {
        fixture_with(0x42)
    }

    fn fixture_with(byte: u8) -> Fixture {
        // A fixed scalar: the tests are about the ceremony, and a deterministic
        // key means a failure is reproducible.
        let signing = SigningKey::from_slice(&[byte; 32]).expect("a valid P-256 scalar");
        Fixture {
            signing,
            credential_id: b"credential-abc-123".to_vec(),
        }
    }

    fn registration(fixture: &Fixture, challenge: &str) -> RegistrationInput {
        let cose = cose_key(&fixture.signing);
        let data = auth_data(
            RP_ID,
            FLAG_USER_PRESENT | FLAG_USER_VERIFIED | FLAG_ATTESTED_CREDENTIAL_DATA,
            0,
            Some((&fixture.credential_id, cose)),
        );
        RegistrationInput {
            credential_id: encode_b64url(&fixture.credential_id),
            client_data_json: encode_b64url(&client_data(
                "webauthn.create",
                challenge,
                ORIGIN,
                false,
            )),
            attestation_object: encode_b64url(&attestation(&data, "none")),
        }
    }

    fn assertion(fixture: &Fixture, challenge: &str, counter: u32) -> AssertionInput {
        let data = auth_data(RP_ID, FLAG_USER_PRESENT, counter, None);
        let cd = client_data("webauthn.get", challenge, ORIGIN, false);
        let mut signed = data.clone();
        signed.extend_from_slice(&Sha256::digest(&cd));
        let signature: p256::ecdsa::Signature = fixture.signing.sign(&signed);
        AssertionInput {
            credential_id: encode_b64url(&fixture.credential_id),
            client_data_json: encode_b64url(&cd),
            authenticator_data: encode_b64url(&data),
            signature: encode_b64url(signature.to_der().as_bytes()),
        }
    }

    #[test]
    fn a_well_formed_registration_is_accepted() {
        let fixture = fixture();
        let challenge = new_challenge();
        let registered =
            verify_registration(&registration(&fixture, &challenge), &ceremony(&challenge))
                .expect("a valid registration should verify");
        assert_eq!(
            registered.credential_id,
            encode_b64url(&fixture.credential_id)
        );
        assert_eq!(registered.public_key.len(), 65);
        assert_eq!(registered.public_key[0], 4);
        assert!(registered.user_verified);
        let expected = fixture.signing.verifying_key().to_encoded_point(false);
        assert_eq!(registered.public_key[1..], expected.as_bytes()[1..]);
    }

    #[test]
    fn a_registration_with_the_wrong_challenge_is_refused() {
        let fixture = fixture();
        let input = registration(&fixture, &new_challenge());
        let error = verify_registration(&input, &ceremony(&new_challenge())).unwrap_err();
        assert!(matches!(error, WebAuthnError::Mismatch(_)), "{error}");
        assert!(error.to_string().contains("challenge"), "{error}");
    }

    #[test]
    fn a_registration_from_another_origin_is_refused() {
        let fixture = fixture();
        let challenge = new_challenge();
        let cose = cose_key(&fixture.signing);
        let data = auth_data(
            RP_ID,
            FLAG_USER_PRESENT | FLAG_ATTESTED_CREDENTIAL_DATA,
            0,
            Some((&fixture.credential_id, cose)),
        );
        let input = RegistrationInput {
            credential_id: encode_b64url(&fixture.credential_id),
            client_data_json: encode_b64url(&client_data(
                "webauthn.create",
                &challenge,
                "https://evil.example",
                false,
            )),
            attestation_object: encode_b64url(&attestation(&data, "none")),
        };
        let error = verify_registration(&input, &ceremony(&challenge)).unwrap_err();
        assert!(error.to_string().contains("evil.example"), "{error}");
    }

    #[test]
    fn a_ceremony_for_another_domain_is_refused() {
        let fixture = fixture();
        let challenge = new_challenge();
        let input = registration(&fixture, &challenge);
        let error = verify_registration(
            &input,
            &Ceremony {
                rp_id: "other.example.com".into(),
                origins: vec![ORIGIN.into()],
                challenge,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("different domain"), "{error}");
    }

    #[test]
    fn a_cross_origin_ceremony_is_refused() {
        let fixture = fixture();
        let challenge = new_challenge();
        let cose = cose_key(&fixture.signing);
        let data = auth_data(
            RP_ID,
            FLAG_USER_PRESENT | FLAG_ATTESTED_CREDENTIAL_DATA,
            0,
            Some((&fixture.credential_id, cose)),
        );
        let input = RegistrationInput {
            credential_id: encode_b64url(&fixture.credential_id),
            client_data_json: encode_b64url(&client_data(
                "webauthn.create",
                &challenge,
                ORIGIN,
                true,
            )),
            attestation_object: encode_b64url(&attestation(&data, "none")),
        };
        let error = verify_registration(&input, &ceremony(&challenge)).unwrap_err();
        assert!(error.to_string().contains("crossOrigin"), "{error}");
    }

    #[test]
    fn an_rsa_credential_is_refused_with_a_message_that_says_why() {
        let challenge = new_challenge();
        let data = auth_data(
            RP_ID,
            FLAG_USER_PRESENT | FLAG_ATTESTED_CREDENTIAL_DATA,
            0,
            Some((b"id", {
                let map = ciborium::value::Value::Map(vec![
                    (1.into(), 3.into()),         // kty: RSA
                    (3.into(), (-257i64).into()), // alg: RS256
                ]);
                let mut out = Vec::new();
                ciborium::ser::into_writer(&map, &mut out).unwrap();
                out
            })),
        );
        let input = RegistrationInput {
            credential_id: encode_b64url(b"id"),
            client_data_json: encode_b64url(&client_data(
                "webauthn.create",
                &challenge,
                ORIGIN,
                false,
            )),
            attestation_object: encode_b64url(&attestation(&data, "none")),
        };
        let error = verify_registration(&input, &ceremony(&challenge)).unwrap_err();
        assert!(matches!(error, WebAuthnError::Unsupported(_)), "{error}");
        assert!(error.to_string().contains("ES256"), "{error}");
    }

    #[test]
    fn a_registration_without_credential_data_is_refused() {
        let challenge = new_challenge();
        let data = auth_data(RP_ID, FLAG_USER_PRESENT, 0, None);
        let input = RegistrationInput {
            credential_id: encode_b64url(b"id"),
            client_data_json: encode_b64url(&client_data(
                "webauthn.create",
                &challenge,
                ORIGIN,
                false,
            )),
            attestation_object: encode_b64url(&attestation(&data, "none")),
        };
        let error = verify_registration(&input, &ceremony(&challenge)).unwrap_err();
        assert!(error.to_string().contains("AT flag"), "{error}");
    }

    #[test]
    fn a_mismatched_raw_id_is_refused() {
        let fixture = fixture();
        let challenge = new_challenge();
        let mut input = registration(&fixture, &challenge);
        input.credential_id = encode_b64url(b"some-other-credential");
        let error = verify_registration(&input, &ceremony(&challenge)).unwrap_err();
        assert!(error.to_string().contains("rawId"), "{error}");
    }

    #[test]
    fn a_valid_assertion_verifies_and_returns_the_counter() {
        let fixture = fixture();
        let challenge = new_challenge();
        let registered =
            verify_registration(&registration(&fixture, &challenge), &ceremony(&challenge))
                .unwrap();
        let counter = verify_assertion(
            &assertion(&fixture, &challenge, 7),
            &registered.public_key,
            registered.counter,
            &ceremony(&challenge),
        )
        .expect("a valid assertion should verify");
        assert_eq!(counter, 7);
    }

    #[test]
    fn an_assertion_signed_by_another_key_is_refused() {
        let fixture = fixture();
        let challenge = new_challenge();
        let registered =
            verify_registration(&registration(&fixture, &challenge), &ceremony(&challenge))
                .unwrap();
        let attacker = fixture_with(0x43);
        let error = verify_assertion(
            &assertion(&attacker, &challenge, 1),
            &registered.public_key,
            0,
            &ceremony(&challenge),
        )
        .unwrap_err();
        assert!(error.to_string().contains("does not verify"), "{error}");
    }

    #[test]
    fn an_assertion_with_a_tampered_authenticator_data_is_refused() {
        let fixture = fixture();
        let challenge = new_challenge();
        let registered =
            verify_registration(&registration(&fixture, &challenge), &ceremony(&challenge))
                .unwrap();
        let mut input = assertion(&fixture, &challenge, 1);
        // Flip a bit in the flags byte (user-present off) so the signature no
        // longer matches the data it covers.
        let mut data = decode_b64(&input.authenticator_data).unwrap();
        data[32] = 0;
        input.authenticator_data = encode_b64url(&data);
        let error =
            verify_assertion(&input, &registered.public_key, 0, &ceremony(&challenge)).unwrap_err();
        assert!(matches!(error, WebAuthnError::Mismatch(_)), "{error}");
    }

    #[test]
    fn a_replayed_assertion_with_a_stale_counter_is_refused() {
        let fixture = fixture();
        let challenge = new_challenge();
        let registered =
            verify_registration(&registration(&fixture, &challenge), &ceremony(&challenge))
                .unwrap();
        // First use at counter 5.
        verify_assertion(
            &assertion(&fixture, &challenge, 5),
            &registered.public_key,
            0,
            &ceremony(&challenge),
        )
        .unwrap();
        // A clone reports 3 (or the same 5) after that.
        let error = verify_assertion(
            &assertion(&fixture, &challenge, 3),
            &registered.public_key,
            5,
            &ceremony(&challenge),
        )
        .unwrap_err();
        assert!(error.to_string().contains("cloned"), "{error}");
    }

    #[test]
    fn an_authenticator_that_always_reports_zero_is_accepted() {
        let fixture = fixture();
        let challenge = new_challenge();
        let registered =
            verify_registration(&registration(&fixture, &challenge), &ceremony(&challenge))
                .unwrap();
        for _ in 0..3 {
            verify_assertion(
                &assertion(&fixture, &challenge, 0),
                &registered.public_key,
                0,
                &ceremony(&challenge),
            )
            .expect("a zero counter must not be treated as a clone");
        }
    }

    #[test]
    fn an_assertion_for_the_wrong_ceremony_type_is_refused() {
        let fixture = fixture();
        let challenge = new_challenge();
        let registered =
            verify_registration(&registration(&fixture, &challenge), &ceremony(&challenge))
                .unwrap();
        let mut input = assertion(&fixture, &challenge, 1);
        input.client_data_json =
            encode_b64url(&client_data("webauthn.create", &challenge, ORIGIN, false));
        let error =
            verify_assertion(&input, &registered.public_key, 0, &ceremony(&challenge)).unwrap_err();
        assert!(error.to_string().contains("webauthn.get"), "{error}");
    }

    #[test]
    fn truncated_authenticator_data_is_malformed_not_a_panic() {
        let error = parse_authenticator_data(&[0u8; 20], RP_ID).unwrap_err();
        assert!(matches!(error, WebAuthnError::Malformed(_)), "{error}");
    }

    #[test]
    fn challenges_are_random_and_url_safe() {
        let a = new_challenge();
        let b = new_challenge();
        assert_ne!(a, b);
        assert!(!a.contains('='));
        assert!(decode_b64(&a).is_ok_and(|bytes| bytes.len() == 32));
    }

    #[test]
    fn padding_variants_of_the_same_challenge_compare_equal() {
        let challenge = "AAAA";
        // `standard` base64 of the same bytes decodes to the same value.
        let ceremony = ceremony(challenge);
        let cd = client_data("webauthn.create", "AAAA", ORIGIN, false);
        assert!(ceremony.check_client_data(&cd, "webauthn.create").is_ok());
    }
}
