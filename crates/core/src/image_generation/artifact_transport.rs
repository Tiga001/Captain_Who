//! Artifact-only HTTP transport. A proxy chooses the route, never the artifact destination.
//!
//! Each tunnel addresses one IP already approved by the caller. TLS and HTTP still authenticate
//! the original URL hostname. No Provider credentials, cookies, automatic redirects or automatic
//! direct fallback cross this boundary.

use super::{ImageArtifactError, ImageArtifactErrorCode, ImageArtifactStoreConfig};
use crate::network::NetworkRoute;
use http_body_util::{BodyExt, Empty};
use hyper::body::{Bytes, Incoming};
use hyper_util::rt::TokioIo;
use reqwest::header::{HeaderMap, ACCEPT, ACCEPT_ENCODING, CONNECTION, CONTENT_LENGTH, HOST};
use reqwest::{StatusCode, Url};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, OnceLock};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio_rustls::rustls::{self, pki_types::ServerName};

trait NetworkStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> NetworkStream for T {}
type Stream = Box<dyn NetworkStream>;

pub(super) struct ArtifactHttpResponse {
    response: hyper::Response<Incoming>,
    // A cancelled download must also close its socket, not leave a detached HTTP driver alive.
    driver: JoinHandle<()>,
}

impl Drop for ArtifactHttpResponse {
    fn drop(&mut self) {
        self.driver.abort();
    }
}

impl ArtifactHttpResponse {
    pub(super) fn status(&self) -> StatusCode {
        self.response.status()
    }

    pub(super) fn headers(&self) -> &HeaderMap {
        self.response.headers()
    }

    pub(super) fn content_length(&self) -> Option<u64> {
        self.headers()
            .get(CONTENT_LENGTH)?
            .to_str()
            .ok()?
            .parse()
            .ok()
    }

    pub(super) async fn next_chunk(&mut self) -> Result<Option<Bytes>, ImageArtifactError> {
        while let Some(frame) = self.response.body_mut().frame().await {
            let frame = frame.map_err(|_| transport_error("response body", true))?;
            if let Ok(bytes) = frame.into_data() {
                return Ok(Some(bytes));
            }
        }
        Ok(None)
    }
}

struct DriverGuard(Option<JoinHandle<()>>);

impl Drop for DriverGuard {
    fn drop(&mut self) {
        if let Some(driver) = &self.0 {
            driver.abort();
        }
    }
}

/// Addresses are validated before calling this function. They are never replaced with a second
/// hostname lookup by the proxy. The caller owns the overall deadline and cancellation scope.
pub(super) async fn get_pinned(
    url: &Url,
    addresses: &[SocketAddr],
    route: &NetworkRoute,
    accept: &'static str,
    config: ImageArtifactStoreConfig,
) -> Result<ArtifactHttpResponse, ImageArtifactError> {
    get_pinned_with_tls_config(url, addresses, route, accept, config, tls_config()).await
}

async fn get_pinned_with_tls_config(
    url: &Url,
    addresses: &[SocketAddr],
    route: &NetworkRoute,
    accept: &'static str,
    config: ImageArtifactStoreConfig,
    tls_config: Arc<rustls::ClientConfig>,
) -> Result<ArtifactHttpResponse, ImageArtifactError> {
    let host = url
        .host_str()
        .ok_or_else(|| transport_error("URL host", false))?;
    let mut last_error = None;
    let mut connected = None;
    // Bound address failover as well as the outer attempt count. The total transfer deadline
    // remains authoritative even if a DNS answer supplies a large set of unreachable addresses.
    for &target in addresses.iter().take(4) {
        if matches!(route, NetworkRoute::Socks4Proxy { .. }) && target.is_ipv6() {
            continue;
        }
        match tokio::time::timeout(
            config.connect_timeout,
            connect_target(target, route, Arc::clone(&tls_config)),
        )
        .await
        {
            Ok(Ok(stream)) => {
                connected = Some(stream);
                break;
            }
            Ok(Err(error)) => {
                if !error.retryable {
                    return Err(error);
                }
                last_error = Some(error);
            }
            Err(_) => last_error = Some(transport_error("connection timeout", true)),
        }
    }
    let mut stream = connected.ok_or_else(|| {
        last_error.unwrap_or_else(|| transport_error("no usable destination", false))
    })?;
    if url.scheme() == "https" {
        stream = tokio::time::timeout(config.connect_timeout, tls(stream, host, tls_config))
            .await
            .map_err(|_| transport_error("origin TLS timeout", true))??;
    }
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|_| transport_error("HTTP handshake", true))?;
    let driver = tokio::spawn(async move {
        let _ = connection.await;
    });
    let mut guard = DriverGuard(Some(driver));
    let authority = &url[url::Position::BeforeHost..url::Position::AfterPort];
    let mut path = url.path().to_string();
    if let Some(query) = url.query() {
        path.push('?');
        path.push_str(query);
    }
    let request = hyper::Request::get(path)
        .header(HOST, authority)
        .header(ACCEPT, accept)
        .header(ACCEPT_ENCODING, "identity")
        .header(CONNECTION, "close")
        .body(Empty::<Bytes>::new())
        .map_err(|_| transport_error("HTTP request", false))?;
    let response = sender
        .send_request(request)
        .await
        .map_err(|_| transport_error("HTTP response", true))?;
    Ok(ArtifactHttpResponse {
        response,
        driver: guard
            .0
            .take()
            .expect("HTTP driver is owned until response handoff"),
    })
}

async fn connect_target(
    target: SocketAddr,
    route: &NetworkRoute,
    tls_config: Arc<rustls::ClientConfig>,
) -> Result<Stream, ImageArtifactError> {
    let (host, port) = match route {
        NetworkRoute::Direct => {
            let socket = TcpStream::connect(target)
                .await
                .map_err(|_| transport_error("direct connection", true))?;
            if socket.peer_addr().ok() != Some(target) {
                return Err(transport_error("direct peer identity", false));
            }
            return Ok(Box::new(socket));
        }
        NetworkRoute::HttpProxy { host, port }
        | NetworkRoute::HttpsProxy { host, port }
        | NetworkRoute::Socks5Proxy { host, port }
        | NetworkRoute::Socks4Proxy { host, port } => (host.as_str(), *port),
    };
    // The Host-owned proxy may itself be loopback/private. This exception is scoped solely to
    // the configured proxy endpoint; the artifact destination keeps its independent IP policy.
    let proxy_addresses = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| transport_error("proxy DNS", true))?
        .collect::<Vec<_>>();
    let mut socket = None;
    for address in proxy_addresses.iter().take(4) {
        if let Ok(candidate) = TcpStream::connect(address).await {
            if !proxy_addresses.contains(
                &candidate
                    .peer_addr()
                    .map_err(|_| transport_error("proxy peer identity", false))?,
            ) {
                return Err(transport_error("proxy peer identity", false));
            }
            socket = Some(candidate);
            break;
        }
    }
    let mut stream: Stream =
        Box::new(socket.ok_or_else(|| transport_error("proxy connection", true))?);
    if matches!(route, NetworkRoute::HttpsProxy { .. }) {
        stream = tls(stream, host, tls_config).await?;
    }
    match route {
        NetworkRoute::HttpProxy { .. } | NetworkRoute::HttpsProxy { .. } => {
            http_connect(&mut stream, target).await?;
        }
        NetworkRoute::Socks5Proxy { .. } => socks5_connect(&mut stream, target).await?,
        NetworkRoute::Socks4Proxy { .. } => socks4_connect(&mut stream, target).await?,
        NetworkRoute::Direct => unreachable!(),
    }
    Ok(stream)
}

fn tls_config() -> Arc<rustls::ClientConfig> {
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    Arc::clone(CONFIG.get_or_init(|| {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        for cert in rustls_native_certs::load_native_certs().certs {
            let _ = roots.add(cert);
        }
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("the bundled TLS provider supports default protocols")
        .with_root_certificates(roots)
        .with_no_client_auth();
        Arc::new(config)
    }))
}

async fn tls(
    stream: Stream,
    host: &str,
    config: Arc<rustls::ClientConfig>,
) -> Result<Stream, ImageArtifactError> {
    let name = ServerName::try_from(host.trim_matches(['[', ']']).to_string())
        .map_err(|_| transport_error("TLS server name", false))?;
    let stream = tokio_rustls::TlsConnector::from(config)
        .connect(name, stream)
        .await
        .map_err(classify_tls_error)?;
    Ok(Box::new(stream))
}

fn classify_tls_error(error: std::io::Error) -> ImageArtifactError {
    let protocol_error = error
        .get_ref()
        .and_then(|source| source.downcast_ref::<rustls::Error>());
    if matches!(
        protocol_error,
        Some(
            rustls::Error::InvalidCertificate(_)
                | rustls::Error::InvalidCertRevocationList(_)
                | rustls::Error::NoCertificatesPresented
                | rustls::Error::UnsupportedNameType
        )
    ) {
        return transport_error("TLS authentication", false);
    }
    if protocol_error.is_some()
        || matches!(
            error.kind(),
            std::io::ErrorKind::InvalidData | std::io::ErrorKind::InvalidInput
        )
    {
        return transport_error("TLS protocol", false);
    }
    // EOF/reset/timeout during the TLS handshake is a transport failure, not evidence of an
    // invalid certificate. Retrying the GET is safe; certificate/protocol failures stay closed.
    transport_error("TLS transport", true)
}

async fn http_connect(stream: &mut Stream, target: SocketAddr) -> Result<(), ImageArtifactError> {
    let request = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|_| transport_error("proxy CONNECT write", true))?;
    let mut response = Vec::new();
    loop {
        // Read exactly through the header terminator so no tunnel bytes are accidentally lost.
        let byte = stream
            .read_u8()
            .await
            .map_err(|_| transport_error("proxy CONNECT response", true))?;
        response.push(byte);
        if response.ends_with(b"\r\n\r\n") {
            break;
        }
        if response.len() >= 16 * 1024 {
            return Err(transport_error("proxy CONNECT header limit", false));
        }
    }
    let line = response
        .split(|&byte| byte == b'\n')
        .next()
        .unwrap_or_default();
    let mut fields = line.split(|&byte| byte == b' ');
    let protocol = fields.next().unwrap_or_default();
    let status = fields.next().unwrap_or_default();
    if !matches!(protocol, b"HTTP/1.0" | b"HTTP/1.1") || status != b"200" {
        return Err(transport_error(
            if status == b"407" {
                "proxy authentication required"
            } else {
                "proxy CONNECT rejected"
            },
            status.starts_with(b"5"),
        ));
    }
    Ok(())
}

async fn socks5_connect(stream: &mut Stream, target: SocketAddr) -> Result<(), ImageArtifactError> {
    stream
        .write_all(&[5, 1, 0])
        .await
        .map_err(|_| transport_error("SOCKS greeting", true))?;
    let mut greeting = [0; 2];
    stream
        .read_exact(&mut greeting)
        .await
        .map_err(|_| transport_error("SOCKS greeting", true))?;
    if greeting != [5, 0] {
        return Err(transport_error("SOCKS authentication required", false));
    }
    let mut request = vec![5, 1, 0];
    match target.ip() {
        IpAddr::V4(ip) => {
            request.push(1);
            request.extend(ip.octets());
        }
        IpAddr::V6(ip) => {
            request.push(4);
            request.extend(ip.octets());
        }
    }
    request.extend(target.port().to_be_bytes());
    stream
        .write_all(&request)
        .await
        .map_err(|_| transport_error("SOCKS CONNECT write", true))?;
    let mut reply = [0; 4];
    stream
        .read_exact(&mut reply)
        .await
        .map_err(|_| transport_error("SOCKS CONNECT response", true))?;
    if reply[0] != 5 || reply[1] != 0 || reply[2] != 0 {
        return Err(transport_error(
            "SOCKS CONNECT rejected",
            matches!(reply[1], 3..=6),
        ));
    }
    let length = match reply[3] {
        1 => 4,
        4 => 16,
        3 => usize::from(
            stream
                .read_u8()
                .await
                .map_err(|_| transport_error("SOCKS reply address", false))?,
        ),
        _ => return Err(transport_error("SOCKS reply address", false)),
    };
    let mut bound = vec![0; length + 2];
    stream
        .read_exact(&mut bound)
        .await
        .map_err(|_| transport_error("SOCKS reply address", true))?;
    Ok(())
}

async fn socks4_connect(stream: &mut Stream, target: SocketAddr) -> Result<(), ImageArtifactError> {
    let IpAddr::V4(ip) = target.ip() else {
        return Err(transport_error(
            "SOCKS4 requires an IPv4 destination",
            false,
        ));
    };
    let mut request = vec![4, 1];
    request.extend(target.port().to_be_bytes());
    request.extend(ip.octets());
    request.push(0); // Empty USERID. Never send the URL host via SOCKS4a remote DNS.
    stream
        .write_all(&request)
        .await
        .map_err(|_| transport_error("SOCKS4 CONNECT write", true))?;
    let mut reply = [0; 8];
    stream
        .read_exact(&mut reply)
        .await
        .map_err(|_| transport_error("SOCKS4 CONNECT response", true))?;
    if reply[0] != 0 || reply[1] != 90 {
        return Err(transport_error("SOCKS4 CONNECT rejected", false));
    }
    Ok(())
}

fn transport_error(stage: &'static str, retryable: bool) -> ImageArtifactError {
    // Never include an upstream error or URL: artifact query strings contain signed capabilities.
    eprintln!("image Artifact network failure: stage={stage} retryable={retryable}");
    ImageArtifactError::new(
        ImageArtifactErrorCode::TransportFailed,
        format!("image Artifact transport failed at {stage}"),
        retryable,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use std::time::Duration;
    use tokio::net::TcpListener;

    // Public fixture credentials for the in-process test server only. Production still loads
    // native/WebPKI roots and never accepts this certificate or an invalid hostname.
    fn test_tls() -> (Arc<rustls::ClientConfig>, Arc<rustls::ServerConfig>) {
        let cert = CertificateDer::from(include_bytes!("testdata/artifact-test-cert.der").to_vec());
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            include_bytes!("testdata/artifact-test-key.der").to_vec(),
        ));
        let mut roots = rustls::RootCertStore::empty();
        roots.add(cert.clone()).unwrap();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let client = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let server = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .unwrap();
        (Arc::new(client), Arc::new(server))
    }

    async fn read_headers(stream: &mut (impl AsyncRead + Unpin)) -> String {
        let mut bytes = Vec::new();
        while !bytes.ends_with(b"\r\n\r\n") {
            bytes.push(stream.read_u8().await.unwrap());
            assert!(bytes.len() < 16 * 1024);
        }
        String::from_utf8(bytes).unwrap()
    }

    async fn serves_origin(mut stream: Stream, server: Arc<rustls::ServerConfig>) {
        let mut tls = tokio_rustls::TlsAcceptor::from(server)
            .accept(&mut stream)
            .await
            .unwrap();
        assert_eq!(tls.get_ref().1.server_name(), Some("artifact.invalid"));
        let request = read_headers(&mut tls).await.to_ascii_lowercase();
        assert!(
            request.starts_with("get /image?signature=secret http/1.1\r\n"),
            "{request}"
        );
        assert!(
            request.contains("\r\nhost: artifact.invalid\r\n"),
            "{request}"
        );
        assert!(!request.contains("authorization:"));
        assert!(!request.contains("cookie:"));
        tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nContent-Type: image/png\r\nConnection: close\r\n\r\npng").await.unwrap();
        tls.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn http_and_https_connect_pin_the_ip_but_keep_origin_tls_and_host() {
        for secure_proxy in [false, true] {
            let (client, server) = test_tls();
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let worker = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let mut stream: Stream = if secure_proxy {
                    let proxy_tls = tokio_rustls::TlsAcceptor::from(Arc::clone(&server))
                        .accept(socket)
                        .await
                        .unwrap();
                    assert_eq!(proxy_tls.get_ref().1.server_name(), Some("localhost"));
                    Box::new(proxy_tls)
                } else {
                    Box::new(socket)
                };
                let connect = read_headers(&mut stream).await;
                assert_eq!(
                    connect,
                    "CONNECT 8.8.8.8:443 HTTP/1.1\r\nHost: 8.8.8.8:443\r\n\r\n"
                );
                stream
                    .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                    .await
                    .unwrap();
                serves_origin(stream, server).await;
            });
            let route = if secure_proxy {
                NetworkRoute::HttpsProxy {
                    host: "localhost".into(),
                    port,
                }
            } else {
                NetworkRoute::HttpProxy {
                    host: "127.0.0.1".into(),
                    port,
                }
            };
            let mut response = get_pinned_with_tls_config(
                &Url::parse("https://artifact.invalid/image?signature=secret").unwrap(),
                &["8.8.8.8:443".parse().unwrap()],
                &route,
                "image/png",
                ImageArtifactStoreConfig::default(),
                client,
            )
            .await
            .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.next_chunk().await.unwrap().unwrap(), "png");
            assert!(response.next_chunk().await.unwrap().is_none());
            worker.await.unwrap();
        }
    }

    #[tokio::test]
    async fn socks_uses_ip_address_frames_without_proxy_dns_and_preserves_tls() {
        for (version, target) in [
            (4, "8.8.8.8:443"),
            (5, "8.8.8.8:443"),
            (5, "[2606:4700:4700::1111]:443"),
        ] {
            let (client, server) = test_tls();
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let target: SocketAddr = target.parse().unwrap();
            let worker = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                if version == 4 {
                    let mut request = [0; 9];
                    socket.read_exact(&mut request).await.unwrap();
                    assert_eq!(request, [4, 1, 1, 187, 8, 8, 8, 8, 0]);
                    socket.write_all(&[0, 90, 0, 0, 0, 0, 0, 0]).await.unwrap();
                } else {
                    let mut greeting = [0; 3];
                    socket.read_exact(&mut greeting).await.unwrap();
                    assert_eq!(greeting, [5, 1, 0]);
                    socket.write_all(&[5, 0]).await.unwrap();
                    let mut request = vec![0; if target.is_ipv4() { 10 } else { 22 }];
                    socket.read_exact(&mut request).await.unwrap();
                    assert_eq!(&request[..3], &[5, 1, 0]);
                    match target.ip() {
                        IpAddr::V4(ip) => {
                            assert_eq!(request[3], 1);
                            assert_eq!(&request[4..8], &ip.octets());
                        }
                        IpAddr::V6(ip) => {
                            assert_eq!(request[3], 4);
                            assert_eq!(&request[4..20], &ip.octets());
                        }
                    }
                    assert_eq!(&request[request.len() - 2..], &[1, 187]);
                    socket
                        .write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0])
                        .await
                        .unwrap();
                }
                serves_origin(Box::new(socket), server).await;
            });
            let route = if version == 4 {
                NetworkRoute::Socks4Proxy {
                    host: "127.0.0.1".into(),
                    port,
                }
            } else {
                NetworkRoute::Socks5Proxy {
                    host: "127.0.0.1".into(),
                    port,
                }
            };
            let mut response = get_pinned_with_tls_config(
                &Url::parse("https://artifact.invalid/image?signature=secret").unwrap(),
                &[target],
                &route,
                "image/png",
                ImageArtifactStoreConfig::default(),
                client,
            )
            .await
            .unwrap();
            assert_eq!(response.next_chunk().await.unwrap().unwrap(), "png");
            worker.await.unwrap();
        }
    }

    #[tokio::test]
    async fn proxy_rejection_never_falls_back_to_the_reachable_direct_target() {
        let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let route = NetworkRoute::HttpProxy {
            host: "127.0.0.1".into(),
            port: proxy.local_addr().unwrap().port(),
        };
        let worker = tokio::spawn(async move {
            let (mut socket, _) = proxy.accept().await.unwrap();
            read_headers(&mut socket).await;
            socket
                .write_all(
                    b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\n\r\n",
                )
                .await
                .unwrap();
        });
        let result = get_pinned(
            &Url::parse("https://artifact.invalid/image?signature=secret").unwrap(),
            &[target.local_addr().unwrap()],
            &route,
            "image/png",
            ImageArtifactStoreConfig::default(),
        )
        .await;
        let error = result.err().unwrap();
        assert!(error.message.contains("proxy authentication required"));
        assert!(!error.retryable);
        assert!(!error.message.contains("signature"));
        assert!(
            tokio::time::timeout(Duration::from_millis(30), target.accept())
                .await
                .is_err()
        );
        worker.await.unwrap();
    }

    #[tokio::test]
    async fn untrusted_origin_certificate_is_rejected_inside_an_approved_tunnel() {
        let (_, server) = test_tls();
        let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let route = NetworkRoute::HttpProxy {
            host: "127.0.0.1".into(),
            port: proxy.local_addr().unwrap().port(),
        };
        let worker = tokio::spawn(async move {
            let (mut socket, _) = proxy.accept().await.unwrap();
            read_headers(&mut socket).await;
            socket.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await.unwrap();
            assert!(tokio_rustls::TlsAcceptor::from(server)
                .accept(socket)
                .await
                .is_err());
        });
        let error = get_pinned(
            &Url::parse("https://artifact.invalid/").unwrap(),
            &["8.8.8.8:443".parse().unwrap()],
            &route,
            "image/png",
            ImageArtifactStoreConfig::default(),
        )
        .await
        .err()
        .unwrap();
        assert!(error.message.contains("TLS authentication"));
        assert!(!error.retryable);
        worker.await.unwrap();
    }

    #[tokio::test]
    async fn trusted_certificate_for_a_different_hostname_is_still_rejected() {
        let (client, server) = test_tls();
        let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let route = NetworkRoute::HttpProxy {
            host: "127.0.0.1".into(),
            port: proxy.local_addr().unwrap().port(),
        };
        let worker = tokio::spawn(async move {
            let (mut socket, _) = proxy.accept().await.unwrap();
            read_headers(&mut socket).await;
            socket.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await.unwrap();
            assert!(tokio_rustls::TlsAcceptor::from(server)
                .accept(socket)
                .await
                .is_err());
        });
        let error = get_pinned_with_tls_config(
            &Url::parse("https://wrong.invalid/").unwrap(),
            &["8.8.8.8:443".parse().unwrap()],
            &route,
            "image/png",
            ImageArtifactStoreConfig::default(),
            client,
        )
        .await
        .err()
        .unwrap();
        assert!(error.message.contains("TLS authentication"));
        assert!(!error.retryable);
        worker.await.unwrap();
    }

    #[tokio::test]
    async fn dropping_response_closes_the_incomplete_download_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = listener.local_addr().unwrap();
        let worker = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_headers(&mut socket).await;
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 9999\r\n\r\nx")
                .await
                .unwrap();
            let mut remaining = [0; 16];
            let closed = tokio::time::timeout(Duration::from_secs(2), socket.read(&mut remaining))
                .await
                .unwrap();
            assert!(matches!(closed, Ok(0)) || closed.is_err());
        });
        let response = get_pinned(
            &Url::parse("http://artifact.invalid/").unwrap(),
            &[target],
            &NetworkRoute::Direct,
            "image/png",
            ImageArtifactStoreConfig::default(),
        )
        .await
        .unwrap();
        drop(response);
        worker.await.unwrap();
    }

    #[tokio::test]
    async fn eof_during_tls_handshake_is_retryable_without_relaxing_certificate_validation() {
        let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let route = NetworkRoute::HttpProxy {
            host: "127.0.0.1".into(),
            port: proxy.local_addr().unwrap().port(),
        };
        let worker = tokio::spawn(async move {
            let (mut socket, _) = proxy.accept().await.unwrap();
            read_headers(&mut socket).await;
            socket.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await.unwrap();
            // Read the ClientHello, then close without sending any TLS certificate/ServerHello.
            let mut hello = [0; 4096];
            assert!(socket.read(&mut hello).await.unwrap() > 0);
            socket.shutdown().await.unwrap();
        });
        let error = get_pinned(
            &Url::parse("https://artifact.invalid/").unwrap(),
            &["8.8.8.8:443".parse().unwrap()],
            &route,
            "image/png",
            ImageArtifactStoreConfig::default(),
        )
        .await
        .err()
        .unwrap();
        assert!(error.message.contains("TLS transport"));
        assert!(error.retryable);
        worker.await.unwrap();
    }
}
