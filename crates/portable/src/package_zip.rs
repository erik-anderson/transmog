//! Case-specific namespace over the shared bounded Stored ZIP64 implementation.
use crate::{Error, invalid};
use std::{
    collections::BTreeMap,
    io::{Read, Seek, Write},
    sync::atomic::AtomicBool,
};
pub(super) use transmog_package_zip::Member;
fn check(name: &str) -> Result<(), Error> {
    if name != "manifest.json"
        && !name
            .strip_prefix("objects/sha256/")
            .is_some_and(crate::valid_digest)
    {
        return Err(invalid("Unexpected or unsafe ZIP member"));
    }
    Ok(())
}
pub(super) fn read(
    input: &mut (impl Read + Seek + ?Sized),
    canceled: &AtomicBool,
) -> Result<BTreeMap<String, Member>, Error> {
    let members = transmog_package_zip::read(input, canceled)?;
    for name in members.keys() {
        check(name)?;
    }
    Ok(members)
}
pub(super) struct Writer<W>(transmog_package_zip::Writer<W>);
impl<W: Write> Writer<W> {
    pub fn new(output: W) -> Self {
        Self(transmog_package_zip::Writer::new(output))
    }
    pub fn member(
        &mut self,
        name: &str,
        input: &mut dyn Read,
        expected: u64,
        canceled: &AtomicBool,
    ) -> Result<(), Error> {
        check(name)?;
        Ok(self.0.member(name, input, expected, canceled)?)
    }
    pub fn boundary(
        &mut self,
        action: &mut impl FnMut(&mut W) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.0.boundary(action)
    }
    pub fn finish(self) -> Result<W, Error> {
        Ok(self.0.finish()?)
    }
}
impl From<transmog_package_zip::Error> for Error {
    fn from(error: transmog_package_zip::Error) -> Self {
        match error {
            transmog_package_zip::Error::Invalid(s) => Self::Invalid(s),
            transmog_package_zip::Error::Unsupported(s) => Self::Unsupported(s),
            transmog_package_zip::Error::Canceled => Self::Canceled,
            transmog_package_zip::Error::Io(e) => Self::Io(e),
        }
    }
}
