//! Direction-separated HMAC proofs authenticate both peers without transmitting the capability.
use super::{common::unavailable, wire::Hello};
use amphora_api::{self as api, Refusal};
use hmac::{Hmac, Mac};
use sha2::Sha256;
pub(super) fn nonce() -> Result<[u8; 32], Refusal> {
    let mut nonce = [0; 32];
    getrandom::fill(&mut nonce).map_err(unavailable)?;
    Ok(nonce)
}
/// What a greeting agreed since version 9 (local-owner-transport.md, "Negotiated versions"): the
/// newest version the client said it speaks, as the owner read it, and the version the owner chose.
/// Both proofs bind it once the owner speaks 9, so a greeting whose [`super::wire::Speaks`] or
/// [`super::wire::Agreed`] was cut off on the way fails authentication instead of agreeing on an
/// older version. With a version-8 owner there is none, and the proofs are version 8's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Agreement {
    pub newest: u32,
    pub version: u32,
}
fn context(
    token: &[u8; 32],
    server: bool,
    hello: &Hello,
    nonce: &[u8; 32],
    agreement: Option<Agreement>,
) -> Result<Hmac<Sha256>, Refusal> {
    let mut mac = Hmac::<Sha256>::new_from_slice(token).map_err(unavailable)?;
    mac.update(if server {
        b"amphora.owner.server.v2"
    } else {
        b"amphora.owner.client.v2"
    });
    mac.update(&hello.version.to_be_bytes());
    mac.update(&hello.millis.to_be_bytes());
    mac.update(&hello.nonce);
    mac.update(nonce);
    if let Some(agreement) = agreement {
        mac.update(b"agreed");
        mac.update(&agreement.newest.to_be_bytes());
        mac.update(&agreement.version.to_be_bytes());
    }
    Ok(mac)
}
pub(super) fn proof(
    token: &[u8; 32],
    server: bool,
    hello: &Hello,
    nonce: &[u8; 32],
    agreement: Option<Agreement>,
) -> Result<[u8; 32], Refusal> {
    Ok(context(token, server, hello, nonce, agreement)?
        .finalize()
        .into_bytes()
        .into())
}
pub(super) fn verify(
    token: &[u8; 32],
    server: bool,
    hello: &Hello,
    nonce: &[u8; 32],
    agreement: Option<Agreement>,
    proof: &[u8; 32],
) -> Result<(), Refusal> {
    context(token, server, hello, nonce, agreement)?
        .verify_slice(proof)
        .map_err(|_| api::invalid("owner authentication", "peer proof did not match"))
}
