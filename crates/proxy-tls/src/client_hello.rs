//! Per-connection settings which do not have an SSL context setter.
use boring::{
    error::ErrorStack,
    ex_data::Index,
    ssl::{SslContext, SslContextBuilder, SslRef},
};
use std::sync::OnceLock;

struct EchGrease;
static ECH_GREASE_INDEX: OnceLock<Result<Index<SslContext, EchGrease>, ErrorStack>> =
    OnceLock::new();

pub(crate) fn enable_ech_grease(context: &mut SslContextBuilder) -> Result<(), ErrorStack> {
    let index = ECH_GREASE_INDEX
        .get_or_init(SslContext::new_ex_index::<EchGrease>)
        .clone()?;
    context.set_ex_data(index, EchGrease);
    Ok(())
}

pub(crate) fn handshake_started(ssl: &SslRef) {
    let enabled = ECH_GREASE_INDEX
        .get()
        .and_then(|index| index.as_ref().ok())
        .is_some_and(|index| ssl.ssl_context().ex_data(*index).is_some());
    if enabled && !ssl.is_server() {
        // BoringSSL calls HANDSHAKE_START before constructing ClientHello or
        // selecting ECH. The context marker also survives capture attachment
        // and covers SSL objects created inside Hyper and quiche.
        ssl.set_enable_ech_grease(true);
    }
}
