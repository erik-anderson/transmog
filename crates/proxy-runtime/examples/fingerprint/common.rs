use boring::ssl::{
    AlpnError, SelectCertError, SslAcceptor, SslContextBuilder, SslMethod, SslRef,
    select_next_proto,
};
use serde_json::{Value, json};
use std::{
    fmt::Write,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use transmog_tls::IssuedLeaf;

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Hello = Arc<Mutex<Vec<Vec<u8>>>>;
pub type Record = Value;
pub const LIMIT: usize = 256 * 1024;
pub struct Shared {
    pub directory: PathBuf,
    pub proof: String,
    pub next: AtomicUsize,
}
impl Shared {
    pub fn save(&self, record: &Record) -> Result<(), Error> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let temporary = self.directory.join(format!("connection-{id}.tmp"));
        std::fs::write(&temporary, serde_json::to_vec_pretty(record)?)?;
        std::fs::rename(
            temporary,
            self.directory.join(format!("connection-{id}.json")),
        )?;
        Ok(())
    }
    pub fn body(&self) -> String {
        format!(
            "<!doctype html><title>Local fingerprint probe</title><p>{}</p>",
            self.proof
        )
    }
}
pub fn hello() -> Hello {
    Arc::default()
}
pub fn acceptor(
    leaf: &IssuedLeaf,
    alpn: &'static [u8],
    recorded: &Hello,
) -> Result<SslAcceptor, Error> {
    let mut builder = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls())?;
    builder.set_certificate(&leaf.certificate)?;
    builder.set_private_key(&leaf.private_key)?;
    for issuer in &leaf.issuer_chain {
        builder.add_extra_chain_cert(issuer.clone())?;
    }
    builder.set_alpn_select_callback(move |_, offered| {
        select_next_proto(alpn, offered).ok_or(AlpnError::NOACK)
    });
    record_hello(&mut builder, recorded);
    Ok(builder.build())
}
pub fn record_hello(builder: &mut SslContextBuilder, recorded: &Hello) {
    let output = recorded.clone();
    builder.set_select_certificate_callback(move |hello| {
        let bytes = hello.as_bytes();
        let mut output = output.lock().unwrap();
        if bytes.len() > LIMIT || output.len() >= 8 {
            return Err(SelectCertError::ERROR);
        }
        output.push(bytes.to_vec());
        Ok(())
    });
}
pub fn hex(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(result, "{byte:02x}").expect("writing to String cannot fail");
    }
    result
}
pub fn tls(ssl: &SslRef, recorded: &Hello) -> Value {
    json!({"client_hellos":recorded.lock().unwrap().iter().map(|h|hex(h)).collect::<Vec<_>>(), "version":ssl.version_str(), "cipher":ssl.current_cipher().map(boring::ssl::SslCipherRef::name), "alpn":ssl.selected_alpn_protocol().map(|p| String::from_utf8_lossy(p).into_owned()), "ech_accepted":ssl.ech_accepted()})
}
