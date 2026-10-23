use super::*;
use crate::credentials::SecretStore;

#[derive(Clone, PartialEq, Serialize)]
struct Item(String);
impl Record for Item {
    fn id(&self) -> String {
        self.0.clone()
    }
    fn device_id(&self) -> &str {
        "fixture"
    }
    fn is_visible(&self, _: i64) -> bool {
        true
    }
    fn modified_at_ms(&self) -> i64 {
        0
    }
    fn content(&self) -> String {
        self.0.clone()
    }
}

#[derive(Debug)]
struct NoCertificate;
impl rustls::server::ResolvesServerCert for NoCertificate {
    fn resolve(
        &self,
        _: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        panic!("address projection must not open TLS")
    }
}
#[derive(Default)]
struct Secrets(Mutex<BTreeMap<String, Vec<u8>>>);
impl SecretStore for Secrets {
    fn read(&self, _: &str, account: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.0.lock().unwrap().get(account).cloned())
    }
    fn write(&self, _: &str, account: &str, bytes: &[u8]) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .insert(account.to_owned(), bytes.to_vec());
        Ok(())
    }
}

#[test]
fn connection_projection_follows_address_and_account_without_stale_cache() {
    let directory = std::env::temp_dir().join(format!("lan-address-{}", uuid::Uuid::new_v4()));
    let credentials =
        Credentials::new("fixture", directory.clone(), Arc::new(Secrets::default())).unwrap();
    let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_cert_resolver(Arc::new(NoCertificate));
    let identity = Identity {
        certificate: vec![],
        fingerprint: "fixture-certificate".into(),
        tls: Arc::new(tls),
    };
    let server = LanServer::new(
        "fixture".into(),
        "Fixture".into(),
        credentials,
        Arc::new(Mutex::new(
            SnapshotCache::<Item>::new(
                "fixture".into(),
                "Fixture".into(),
                directory.join("cursor.json"),
            )
            .unwrap(),
        )),
        identity,
        Arc::new(|_, _| Ok(None)),
    );
    assert!(server.set_account(Some("first-account")).unwrap().is_none());
    assert!(server.set_endpoints(Some("https://192.0.2.1:1234".into()), vec![]));
    let first = server.local_connection().unwrap();
    assert!(first.get("alternateEndpoints").is_none());
    assert!(!server.set_endpoints(Some("https://192.0.2.1:1234".into()), vec![]));
    assert!(server.set_endpoints(Some("https://192.0.2.2:1234".into()), vec![]));
    let connection = server.local_connection().unwrap();
    assert_eq!(connection["endpoint"], "https://192.0.2.2:1234");
    assert_eq!(connection["token"], first["token"]);
    // An overlay address coming or going is a change of its own.
    let tailnet = vec!["https://100.69.88.16:1234".to_string()];
    assert!(server.set_endpoints(Some("https://192.0.2.2:1234".into()), tailnet.clone()));
    assert!(!server.set_endpoints(Some("https://192.0.2.2:1234".into()), tailnet.clone()));
    assert_eq!(
        server.local_connection().unwrap()["alternateEndpoints"],
        json!(["https://100.69.88.16:1234"])
    );
    assert_eq!(
        server.status()["alternateEndpoints"],
        json!(["https://100.69.88.16:1234"])
    );
    assert!(server.set_endpoints(Some("https://192.0.2.2:1234".into()), vec![]));
    assert!(server
        .local_connection()
        .unwrap()
        .get("alternateEndpoints")
        .is_none());
    assert!(server.set_endpoints(None, vec![]));
    assert!(server.local_connection().is_none());
    assert!(server.set_endpoints(Some("https://192.0.2.3:1234".into()), vec![]));
    assert_eq!(server.local_connection().unwrap()["token"], first["token"]);
    server.set_account(None).unwrap();
    assert!(server.local_connection().is_none());
    let other = server.set_account(Some("second-account")).unwrap().unwrap();
    assert_ne!(other["token"], first["token"]);
    assert_eq!(other["endpoint"], "https://192.0.2.3:1234");
}

#[test]
fn the_lan_address_leads_and_overlay_addresses_follow_once() {
    let lan = vec!["192.168.1.19".to_string(), "192.168.2.4".to_string()];
    let overlay = vec!["100.69.88.16".to_string()];
    assert_eq!(
        endpoints(&lan, &overlay, 52724),
        (
            Some("https://192.168.1.19:52724".into()),
            vec!["https://100.69.88.16:52724".into()]
        )
    );
    // Without a LAN address the overlay address leads; Windows lists it in both.
    assert_eq!(
        endpoints(&[], &overlay, 52724),
        (Some("https://100.69.88.16:52724".into()), vec![])
    );
    assert_eq!(
        endpoints(&overlay, &overlay, 52724),
        (Some("https://100.69.88.16:52724".into()), vec![])
    );
    assert_eq!(endpoints(&lan, &[], 52724).1, Vec::<String>::new());
    assert_eq!(endpoints(&[], &[], 52724), (None, vec![]));
}
