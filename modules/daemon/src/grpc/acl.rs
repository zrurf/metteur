//! ACL enforcement as a tower layer.
//!
//! The layer runs on the `http::Request` of each RPC, which carries both the
//! method path (via the URI) and the TLS peer certificate (via `TlsConnectInfo`
//! in the request extensions). The caller's subject is the certificate's CN
//! when mTLS is enabled, otherwise `local`.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use metteur_shared::config::AclConfig;
use rustls_pki_types::CertificateDer;
use tonic::Status;
use tonic::body::Body;
use tonic::transport::server::{TcpConnectInfo, TlsConnectInfo};
use tower::Layer;
use tower::Service;

/// The subject used for plaintext (non-TLS) connections.
const LOCAL_SUBJECT: &str = "local";

/// A tower layer that enforces the ACL configuration on each request.
#[derive(Clone)]
pub struct AclLayer {
    acl: Arc<std::sync::RwLock<AclConfig>>,
    /// Reject requests without a client certificate.
    strict: bool,
}

impl AclLayer {
    /// Creates a new ACL layer.
    ///
    /// `acl` is read on every request so that configuration changes take
    /// effect immediately. When `strict`, requests without a client
    /// certificate are rejected.
    pub fn new(acl: Arc<std::sync::RwLock<AclConfig>>, strict: bool) -> Self {
        Self {
            acl,
            strict,
        }
    }
}

impl<S> Layer<S> for AclLayer {
    type Service = AclService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AclService {
            inner,
            acl: self.acl.clone(),
            strict: self.strict,
        }
    }
}

/// A service that checks the request path against the ACL before forwarding.
#[derive(Clone)]
pub struct AclService<S> {
    inner: S,
    acl: Arc<std::sync::RwLock<AclConfig>>,
    strict: bool,
}

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

impl<S, ReqBody> Service<http::Request<ReqBody>> for AclService<S>
where
    S: Service<http::Request<ReqBody>, Response = http::Response<Body>, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
    ReqBody: Send + 'static,
{
    type Response = http::Response<Body>;
    type Error = Infallible;
    type Future = BoxFuture<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: http::Request<ReqBody>) -> Self::Future {
        let path = req.uri().path().to_string();
        let subject = match subject_from_http(&req) {
            Some(subject) => subject,
            None if self.strict => {
                let status = Status::permission_denied("client certificate required");
                return Box::pin(async move { Ok(status.into_http::<Body>()) });
            }
            None => LOCAL_SUBJECT.to_string(),
        };

        if self.acl.read().unwrap().is_allowed(&subject, &path) {
            let mut inner = self.inner.clone();
            Box::pin(async move { inner.call(req).await })
        } else {
            let status = Status::permission_denied(format!(
                "subject '{subject}' is not allowed to call {path}"
            ));
            Box::pin(async move { Ok(status.into_http::<Body>()) })
        }
    }
}

/// Returns the peer certificate of a request, if the connection is TLS.
fn subject_from_http<B>(req: &http::Request<B>) -> Option<String> {
    let tls = req.extensions().get::<TlsConnectInfo<TcpConnectInfo>>()?;
    let certs = tls.peer_certs()?;
    let cert = certs.first()?;
    cert_cn(cert)
}

/// Returns the common name of a parsed client certificate.
pub fn cert_cn(cert: &CertificateDer) -> Option<String> {
    use x509_parser::prelude::*;
    let (_, x509) = parse_x509_certificate(cert.as_ref()).ok()?;
    let cn = x509.subject().iter_common_name().next()?;
    cn.as_str().ok().map(|s| s.to_string())
}

/// Resolves the subject identity of a tonic request from its peer certificate.
pub fn subject_from_request<T>(req: &tonic::Request<T>) -> Option<String> {
    let certs = req.peer_certs()?;
    certs.first().and_then(cert_cn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use metteur_shared::config::AclRule;
    use tower::ServiceExt;

    fn cfg(subject: &str, allow: &[&str]) -> AclConfig {
        AclConfig {
            rules: vec![AclRule {
                subject: subject.to_string(),
                allow: allow.iter().map(|s| s.to_string()).collect(),
                deny: vec![],
            }],
        }
    }

    fn is_allowed_impl(subject: &str, path: &str) -> bool {
        let acl = Arc::new(std::sync::RwLock::new(AclConfig::default()));
        // Mirror of the layer's decision given a pre-resolved subject.
        acl.read().unwrap().is_allowed(subject, path)
    }

    #[tokio::test]
    async fn layer_denies_unlisted_path() {
        let acl =
            Arc::new(std::sync::RwLock::new(cfg("local", &["/metteur.Daemon/ListWorkspaces"])));
        let layer = AclLayer::new(acl, false);

        let svc = tower::service_fn(|_req: http::Request<Body>| async {
            Ok::<_, Infallible>(http::Response::new(Body::empty()))
        });
        let mut svc = layer.layer(svc);

        let allowed = http::Request::builder()
            .uri("/metteur.Daemon/ListWorkspaces")
            .body(Body::empty())
            .unwrap();
        let resp = svc.ready().await.unwrap().call(allowed).await.unwrap();
        assert_eq!(resp.status(), http::StatusCode::OK);

        let denied = http::Request::builder()
            .uri("/metteur.Daemon/OpenWorkspace")
            .body(Body::empty())
            .unwrap();
        let resp = svc.ready().await.unwrap().call(denied).await.unwrap();
        // gRPC status is carried in the `grpc-status` header; permission
        // denied maps to code 7.
        assert_eq!(resp.headers().get("grpc-status").and_then(|v| v.to_str().ok()), Some("7"));
    }

    #[tokio::test]
    async fn blackboard_query_requires_its_rpc_acl() {
        let acl =
            Arc::new(std::sync::RwLock::new(cfg("local", &["/metteur.Daemon/ListWorkspaces"])));
        let layer = AclLayer::new(acl, false);
        let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed = called.clone();
        let inner = tower::service_fn(move |_: http::Request<Body>| {
            observed.store(true, std::sync::atomic::Ordering::SeqCst);
            async { Ok::<_, Infallible>(http::Response::new(Body::empty())) }
        });
        let mut svc = layer.layer(inner);
        for method in ["GetBlackboard", "GetConciergeState", "SendConciergeMessage", "ListOversightReports"] {
            let req = http::Request::builder().uri(format!("/metteur.Daemon/{method}")).body(Body::empty()).unwrap();
            let response = svc.ready().await.unwrap().call(req).await.unwrap();
            assert_eq!(response.headers().get("grpc-status").unwrap(), "7");
        }
        assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn layer_strict_rejects_anonymous() {
        // With strict mode, a request without a peer certificate is rejected
        // regardless of the ACL content.
        let acl = Arc::new(std::sync::RwLock::new(AclConfig::default()));
        let layer = AclLayer::new(acl, true);

        let svc = tower::service_fn(|_req: http::Request<Body>| async {
            Ok::<_, Infallible>(http::Response::new(Body::empty()))
        });
        let mut svc = layer.layer(svc);
        let req = http::Request::builder()
            .uri("/metteur.Daemon/ListWorkspaces")
            .body(Body::empty())
            .unwrap();
        let resp = svc.ready().await.unwrap().call(req).await.unwrap();
        assert_eq!(resp.headers().get("grpc-status").and_then(|v| v.to_str().ok()), Some("7"));
    }

    #[test]
    fn empty_rules_allow_everything() {
        assert!(is_allowed_impl("local", "/metteur.Daemon/OpenWorkspace"));
        assert!(is_allowed_impl("anyone", "/anything"));
    }

    #[test]
    fn resolves_cert_common_name() {
        let key = rcgen::KeyPair::generate().unwrap();
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        params.distinguished_name.push(rcgen::DnType::CommonName, "client1");
        let cert = params.self_signed(&key).unwrap();
        assert_eq!(cert_cn(cert.der()).as_deref(), Some("client1"));
    }
}
