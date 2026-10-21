//! Unsigned PKCS#7 CTLs for isolated matching tests; never installed in OS stores.
pub fn ctl(signature_hash: bool, identifiers: &[Vec<u8>]) -> Vec<u8> {
    fn tlv(tag: u8, value: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        if value.len() < 128 {
            out.push(u8::try_from(value.len()).unwrap());
        } else {
            let bytes = value.len().to_be_bytes();
            let first = bytes.iter().position(|b| *b != 0).unwrap();
            out.push(0x80 | u8::try_from(bytes.len() - first).unwrap());
            out.extend(&bytes[first..]);
        }
        out.extend(value);
        out
    }
    let mut body = tlv(0x30, &tlv(6, &[0x2b, 6, 1, 4, 1, 0x82, 0x37, 10, 3, 30]));
    body.extend(tlv(2, &[1]));
    // Deliberately old: OS distrust must not disappear when NextUpdate has passed.
    body.extend(tlv(0x17, b"000101000000Z"));
    body.extend(tlv(0x17, b"000102000000Z"));
    let algorithm = if signature_hash {
        vec![0x2b, 6, 1, 4, 1, 0x82, 0x37, 10, 11, 15]
    } else {
        vec![0x2b, 0x0e, 3, 2, 26] // SHA-1 certificate thumbprints
    };
    body.extend(tlv(0x30, &tlv(6, &algorithm)));
    let mut entries = Vec::new();
    for hash in identifiers {
        entries.extend(tlv(0x30, &tlv(4, hash)));
    }
    body.extend(tlv(0x30, &entries));
    let mut content = tlv(6, &[0x2b, 6, 1, 4, 1, 0x82, 0x37, 10, 1]);
    content.extend(tlv(0xa0, &tlv(0x30, &body)));
    let mut signed = tlv(2, &[1]);
    signed.extend(tlv(0x31, &[]));
    signed.extend(tlv(0x30, &content));
    signed.extend(tlv(0x31, &[]));
    let mut outer = tlv(6, &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 1, 7, 2]);
    outer.extend(tlv(0xa0, &tlv(0x30, &signed)));
    tlv(0x30, &outer)
}
