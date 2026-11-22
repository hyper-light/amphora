//! The owner transport between machines (CP-T1.9, T-T1.9.R18; docs/design/reshard.md, decision
//! "The transport between machines"): a node's endpoint on a network address, for the pulls of a
//! move's new group and the steps of a transaction across groups on another machine.
//!
//! Authentication is the replicated transport's (hyper-transport's `Tls::mutual`): mutual TLS 1.3,
//! each side presenting a certificate from the deployment's authority and verifying the other's
//! against its roots, with the node provider's groups and suites and no early data. Nothing rests
//! on a shared file. The owner protocol then runs inside the TLS stream unchanged, its greeting's
//! capability exported from the TLS session (RFC 8446 §7.5; RFC 5705), so each connection's
//! proofs bind it to that session and the same greeting serves the loopback endpoint and this one.
//! A network endpoint answers only a node's requests: a pull, a step, or a probe.
use super::{
    Error,
    common::{self, Owner, Pulls, Wire, remaining, unavailable},
    endpoint::{self, Stream},
    wire::{Answer, GREETING_MAX, Request},
};
use amphora_api::{self as api, Refusal};
use hyper_tls::{
    self as tls, ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection,
    StreamOwned, pki_types::ServerName, server::WebPkiClientVerifier,
};
use std::{
    io::{Read, Write},
    net::SocketAddr,
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};

pub use hyper_tls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

/// The application protocol both sides negotiate (RFC 7301), the owner protocol's own name: a peer
/// that speaks another is refused at the handshake. A format value.
pub const ALPN: &[u8] = b"amphora-owner";

/// The exporter label the greeting's capability is derived under (RFC 8446 §7.5): a format value.
const EXPORTER: &[u8] = b"EXPORTER-amphora-owner-capability";

/// A node's credentials, as the replicated transport takes them: its certificate chain (its own
/// first), its private key, and the authority's roots its peers' certificates chain to.
pub struct Credentials {
    pub chain: Vec<CertificateDer<'static>>,
    pub key: PrivateKeyDer<'static>,
    pub roots: Vec<CertificateDer<'static>>,
}

/// The key exchange groups between nodes: the hybrid post-quantum ones only, as hyper-quic's
/// `node_provider` names them for the replicated transport (hyper-raft docs/seal.md §10).
static KX_GROUPS: [&dyn tls::crypto::SupportedKxGroup; 3] = [
    tls::crypto::aws_lc_rs::kx_group::SECP384R1MLKEM1024,
    tls::crypto::aws_lc_rs::kx_group::X25519MLKEM768,
    tls::crypto::aws_lc_rs::kx_group::SECP256R1MLKEM768,
];

/// The TLS 1.3 suites between nodes, as hyper-quic's `node_provider` names them: 256-bit keys only,
/// AES-256-GCM first, then ChaCha20-Poly1305 (RFC 8446 §B.4).
static CIPHER_SUITES: [tls::SupportedCipherSuite; 2] = [
    tls::crypto::aws_lc_rs::cipher_suite::TLS13_AES_256_GCM_SHA384,
    tls::crypto::aws_lc_rs::cipher_suite::TLS13_CHACHA20_POLY1305_SHA256,
];

static PROVIDER: LazyLock<tls::crypto::CryptoProvider> =
    LazyLock::new(|| tls::crypto::CryptoProvider {
        cipher_suites: CIPHER_SUITES.to_vec(),
        kx_groups: KX_GROUPS.to_vec(),
        ..tls::crypto::aws_lc_rs::default_provider()
    });

fn refused(what: impl std::fmt::Display) -> Refusal {
    api::invalid("network credentials", &what.to_string())
}

impl Credentials {
    /// Credentials from PEM files: the chain (its own certificate first), the private key, and
    /// the roots.
    pub fn from_pem_files(
        chain: &std::path::Path,
        key: &std::path::Path,
        roots: &std::path::Path,
    ) -> Result<Self, Refusal> {
        use hyper_tls::pki_types::pem::PemObject;
        let certificates = |path: &std::path::Path| {
            CertificateDer::pem_file_iter(path)
                .map_err(refused)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(refused)
        };
        Ok(Self {
            chain: certificates(chain)?,
            key: PrivateKeyDer::from_pem_file(key).map_err(refused)?,
            roots: certificates(roots)?,
        })
    }

    fn roots(&self) -> Result<RootCertStore, Refusal> {
        let mut roots = RootCertStore::empty();
        for root in &self.roots {
            roots.add(root.clone()).map_err(refused)?;
        }
        if roots.is_empty() {
            return Err(refused("no roots"));
        }
        Ok(roots)
    }

    /// The listening side: its chain presented, a client's required under the roots.
    fn server(&self) -> Result<ServerConfig, Refusal> {
        let verifier = WebPkiClientVerifier::builder_with_provider(self.roots()?, &PROVIDER)
            .build()
            .map_err(refused)?;
        let mut config = ServerConfig::builder_with_provider(&PROVIDER)
            .with_protocol_versions(&[&tls::version::TLS13])
            .map_err(refused)?
            .with_client_cert_verifier(verifier)
            .with_single_cert(self.chain.clone(), self.key.clone_key())
            .map_err(refused)?;
        config.alpn_protocols = vec![ALPN.to_vec()];
        config.max_early_data_size = 0;
        Ok(config)
    }

    /// The dialing side: the server verified under the roots, this node's chain presented.
    fn client(&self) -> Result<ClientConfig, Refusal> {
        let mut config = ClientConfig::builder_with_provider(&PROVIDER)
            .with_protocol_versions(&[&tls::version::TLS13])
            .map_err(refused)?
            .with_root_certificates(self.roots()?)
            .with_client_auth_cert(self.chain.clone(), self.key.clone_key())
            .map_err(refused)?;
        config.alpn_protocols = vec![ALPN.to_vec()];
        config.enable_early_data = false;
        Ok(config)
    }
}

impl Credentials {
    fn duplicate(&self) -> Self {
        Self {
            chain: self.chain.clone(),
            key: self.key.clone_key(),
            roots: self.roots.clone(),
        }
    }
}

/// What a node dials its peers with ([`super::Nodes::network`]). A TLS configuration is a
/// connection's own here (hyper-tls drives each with its configuration borrowed mutably), so each
/// dial builds one from these credentials, checked once when the peers are made.
pub(super) struct Dial(Credentials);

impl std::fmt::Debug for Dial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Dial")
    }
}

impl Dial {
    pub(super) fn new(credentials: &Credentials) -> Result<Self, Refusal> {
        credentials.client()?;
        Ok(Self(credentials.duplicate()))
    }
}

impl<C, F> Wire for StreamOwned<C, Stream, F>
where
    Self: Read + Write,
{
    fn read_within(&mut self, within: Duration) -> std::io::Result<()> {
        self.sock.set_read_timeout(Some(within))
    }
    fn write_within(&mut self, within: Duration) -> std::io::Result<()> {
        self.sock.set_write_timeout(Some(within))
    }
}

fn handshake_error(error: std::io::Error) -> Refusal {
    if matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    ) {
        Refusal::Timeout
    } else {
        unavailable(format_args!("network handshake: {error}"))
    }
}

/// The handshake driven to its end by `until`, then the protocol checked and the greeting's
/// capability exported from the session.
macro_rules! handshake {
    ($stream:expr, $until:expr) => {{
        let stream = $stream;
        while stream.conn.is_handshaking() {
            let within = remaining($until)?;
            stream
                .sock
                .set_read_timeout(Some(within))
                .map_err(unavailable)?;
            stream
                .sock
                .set_write_timeout(Some(within))
                .map_err(unavailable)?;
            stream
                .conn
                .complete_io(&mut stream.sock, &mut stream.config)
                .map_err(handshake_error)?;
        }
        if stream.conn.alpn_protocol() != Some(ALPN) {
            return Err(api::invalid(
                "network handshake",
                "the peer speaks another protocol",
            ));
        }
        stream
            .conn
            .export_keying_material([0u8; 32], EXPORTER, None)
            .map_err(unavailable)?
    }};
}

/// A network endpoint listening at `address` (port 0: the system's choice), its connections
/// authenticated by `credentials`, a step served by `acquire`'s database and a pull by `pulls`.
pub(super) fn listen(
    address: SocketAddr,
    credentials: &Credentials,
    acquire: impl Fn() -> Option<crate::database::Database> + Send + Sync + 'static,
    pulls: Option<Arc<Pulls>>,
) -> Result<(Owner, SocketAddr), Refusal> {
    // Checked once here; each connection builds its own (as [`Dial`] does).
    credentials.server()?;
    let credentials = credentials.duplicate();
    let listener = endpoint::listener(address)?;
    let address = listener.local_addr().map_err(unavailable)?;
    let owner = Owner::accept(
        listener,
        (),
        Arc::new(move |socket, _waits| {
            let started = Instant::now();
            let greeting_until = started.checked_add(GREETING_MAX).ok_or(Refusal::Timeout)?;
            let config = credentials.server()?;
            let connection = ServerConnection::new(&config).map_err(unavailable)?;
            let mut stream = StreamOwned::new(connection, config, socket);
            let token = handshake!(&mut stream, greeting_until);
            serve(&mut stream, &token, &acquire, pulls.as_deref(), started)
        }),
    )?;
    Ok((owner, address))
}

/// One node's request on an authenticated connection: a pull, a step, or a probe. Anything else is
/// the loopback endpoint's to serve, and is refused here.
fn serve<S: Wire>(
    stream: &mut S,
    token: &[u8; 32],
    acquire: &dyn Fn() -> Option<crate::database::Database>,
    pulls: Option<&Pulls>,
    started: Instant,
) -> Result<(), Refusal> {
    let until = common::greet_owner(stream, token, started, GREETING_MAX)?;
    let answer = match common::read(stream, until)? {
        Request::Probe => Ok(Answer::Done),
        Request::Pull(pull) => common::pulled(pulls, &pull, until),
        Request::Step {
            here,
            carried,
            envelope,
        } => match acquire() {
            Some(database) => common::stepped(&database, here, carried, envelope, until),
            None => Err(unavailable("owner is draining")),
        },
        _ => Err(api::invalid(
            "network request",
            "a network endpoint answers only a node's pulls, steps and probes",
        )),
    };
    common::send(
        stream,
        &common::encode(&answer, super::wire::FRAME_MAX)?,
        until,
    )
}

/// `request` to the endpoint at `address`, whose certificate names `name`: one authenticated
/// connection and one request, as at the loopback endpoint.
pub(super) fn request(
    dial: &Dial,
    address: SocketAddr,
    name: &ServerName<'static>,
    request: &Request,
    until: Instant,
) -> Result<Answer, Error> {
    remaining(until)?;
    let auth_until = until.min(
        Instant::now()
            .checked_add(GREETING_MAX)
            .ok_or(Refusal::Timeout)?,
    );
    let socket =
        Stream::connect_timeout(&address, remaining(auth_until)?).map_err(handshake_error)?;
    socket.set_nodelay(true).map_err(unavailable)?;
    let mut config = dial.0.client()?;
    let connection = ClientConnection::new(&mut config, name.clone()).map_err(unavailable)?;
    let mut stream = StreamOwned::new(connection, config, socket);
    let token = (|| -> Result<[u8; 32], Refusal> { Ok(handshake!(&mut stream, auth_until)) })()?;
    common::greet_client(&mut stream, &token, until, auth_until)?;
    common::exchange(&mut stream, request, until)
}

/// A server name as a certificate carries it: a DNS name or an IP address.
pub(super) fn name(name: &str) -> Result<ServerName<'static>, Refusal> {
    ServerName::try_from(name.to_owned()).map_err(refused)
}

/// The deployment's authority in tests: one root, one certificate per name (Ed25519, as the
/// replicated transport's tests issue them).
#[cfg(test)]
pub(crate) struct Pki {
    issuer: rcgen::Issuer<'static, rcgen::KeyPair>,
    root: Vec<u8>,
}

#[cfg(test)]
impl Pki {
    pub(crate) fn new() -> Self {
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
        let root = params.self_signed(&key).unwrap().der().to_vec();
        Self {
            issuer: rcgen::Issuer::new(params, key),
            root,
        }
    }

    /// A node's credentials for `name` under this authority.
    pub(crate) fn credentials(&self, name: &str) -> Credentials {
        let params = rcgen::CertificateParams::new(vec![name.to_owned()]).unwrap();
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
        let certificate = params.signed_by(&key, &self.issuer).unwrap();
        Credentials {
            chain: vec![CertificateDer::from(certificate.der().to_vec())],
            key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
            roots: vec![CertificateDer::from(self.root.clone())],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::groups::{Peers, Pull, Pulled};
    use crate::reshard::NodeId;
    use crate::transport::{HANG, Node, Nodes};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn any() -> SocketAddr {
        "127.0.0.1:0".parse().unwrap()
    }

    fn until() -> Instant {
        Instant::now() + HANG
    }

    fn pull() -> Pull {
        Pull::Export {
            from: crate::reshard::GroupId(0),
            rest: None,
            first: true,
            positions: 1,
        }
    }

    /// A node endpoint at a network address whose pulls are counted and answered `Changed(None)`.
    fn node(pki: &Pki, name: &str) -> (Node, Arc<AtomicUsize>) {
        let served = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&served);
        let node = Node::listen(any(), &pki.credentials(name), move |_, _, _| {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(Pulled::Changed(None))
        })
        .unwrap();
        (node, served)
    }

    /// T-T1.9.R18: a pull crosses a network address under mutual TLS from one authority.
    #[test]
    fn t_t1_9_r18_a_pull_crosses_the_network_under_mutual_tls() {
        let pki = Pki::new();
        let (node, served) = node(&pki, "node-1");
        let address = node.address().unwrap();
        let nodes = Nodes::network(
            [(NodeId(1), address, "node-1".to_owned())],
            &pki.credentials("node-0"),
        )
        .unwrap();
        for _ in 0..3 {
            assert_eq!(
                nodes.pull(NodeId(1), &pull(), until()).unwrap(),
                Pulled::Changed(None)
            );
        }
        assert_eq!(served.load(Ordering::SeqCst), 3);
    }

    /// T-T1.9.R18: a client from another authority, a server from another authority, and a
    /// server whose certificate names another node are each refused before any request is served.
    #[test]
    fn t_t1_9_r18_a_peer_outside_the_authority_or_misnamed_is_refused() {
        let pki = Pki::new();
        let other = Pki::new();
        let (listening, served) = node(&pki, "node-1");
        let address = listening.address().unwrap();
        let stranger = Nodes::network(
            [(NodeId(1), address, "node-1".to_owned())],
            &other.credentials("node-0"),
        )
        .unwrap();
        assert!(stranger.pull(NodeId(1), &pull(), until()).is_err());
        let misnamed = Nodes::network(
            [(NodeId(1), address, "node-2".to_owned())],
            &pki.credentials("node-0"),
        )
        .unwrap();
        assert!(misnamed.pull(NodeId(1), &pull(), until()).is_err());
        let (impostor, impostor_served) = node(&other, "node-1");
        let fooled = Nodes::network(
            [(NodeId(1), impostor.address().unwrap(), "node-1".to_owned())],
            &pki.credentials("node-0"),
        )
        .unwrap();
        assert!(fooled.pull(NodeId(1), &pull(), until()).is_err());
        assert_eq!(served.load(Ordering::SeqCst), 0);
        assert_eq!(impostor_served.load(Ordering::SeqCst), 0);
    }

    /// T-T1.9.R18: a network endpoint answers a node's requests only; a database request is the
    /// loopback endpoint's, refused here.
    #[test]
    fn t_t1_9_r18_a_network_endpoint_answers_only_a_nodes_requests() {
        let pki = Pki::new();
        let (node, _served) = node(&pki, "node-1");
        let dial = Dial::new(&pki.credentials("node-0")).unwrap();
        let address = node.address().unwrap();
        let named = name("node-1").unwrap();
        assert!(matches!(
            request(&dial, address, &named, &Request::Probe, until()),
            Ok(Answer::Done)
        ));
        assert!(matches!(
            request(&dial, address, &named, &Request::Identity, until()),
            Err(Error::Refused(_))
        ));
        assert!(matches!(
            request(&dial, address, &named, &Request::Session, until()),
            Err(Error::Refused(_))
        ));
    }
}
