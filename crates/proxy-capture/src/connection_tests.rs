use super::*;
use crate::connections;
use serde_json::json;
use std::io::Cursor;
use transmog_core::{
    TlsEvidence,
    performance::{ConnectionTiming, PerformanceEvidence, TransportObservation},
};

fn evidence() -> PerformanceEvidence {
    PerformanceEvidence {
        transports: vec![TransportObservation {
            leg: "upstream".into(),
            connection_id: "shared-h2-connection".into(),
            peer: Some("203.0.113.1:443".into()),
            tls_version: Some("TLSv1.3".into()),
            tls: Some(TlsEvidence {
                http2_alps: Some(transmog_core::Http2AlpsEvidence {
                    negotiated: true,
                    peer_settings: vec![0, 0, 6, 4, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0].into(),
                }),
                server_name: Some("private.example".into()),
                offered_ciphers: (0..32)
                    .map(|i| format!("TLS_SYNTHETIC_CIPHER_{i:03}"))
                    .collect(),
                offered_alpn: vec!["h2".into(), "http/1.1".into()],
                trust_source: Some("recorded trust snapshot".into()),
                trust_generation: Some(123),
                details: std::collections::BTreeMap::from([
                    ("Negotiated group".into(), "X25519MLKEM768".into()),
                    ("Negotiated group ID".into(), "0x11ec".into()),
                    (
                        "Key exchange".into(),
                        "Hybrid post-quantum key exchange".into(),
                    ),
                    ("Peer handshake signature".into(), "mldsa44".into()),
                ]),
                ..Default::default()
            }),
            ..Default::default()
        }],
        ..Default::default()
    }
}
fn record(id: u128, evidence: PerformanceEvidence) -> CaptureRecord {
    CaptureRecord {
        sequence: 7,
        exchange_id: id,
        kind: CaptureRecordKind::Performance(evidence),
    }
}
fn wire_records(record: &CaptureRecord) -> (Vec<CaptureRecord>, CaptureRecord) {
    let CaptureRecordKind::Performance(evidence) = &record.kind else {
        panic!()
    };
    connections::split(record, evidence).unwrap()
}
fn definition(record: &CaptureRecord) -> bool {
    matches!(&record.kind, CaptureRecordKind::Unknown {kind,..} if kind == connections::DEFINITION_KIND)
}

#[test]
fn shared_contexts_preserve_changing_samples_and_random_reads() {
    for password in [None, Some(CapturePassword::new("context secret".into()))] {
        let mut writer = CaptureWriter::with_encoding(
            Vec::new(),
            CaptureLimits::default(),
            &CaptureEncoding {
                password: password.clone(),
            },
        )
        .unwrap();
        let mut expected = Vec::new();
        for id in 1..=12 {
            let mut sample = evidence();
            sample.transports[0].sampled_offset_micros = id * 11;
            sample.transports[0].bytes_read = Some(u64::try_from(id).unwrap() * 123);
            sample.transports[0].shared = id > 1;
            sample.transports[0].setup_timings = vec![ConnectionTiming {
                phase: "tls".into(),
                began_offset_micros: -id * 100,
                ended_offset_micros: -id * 100 + 10,
                request_wait_micros: 0,
            }];
            // A changed fact for the same connection must not rewrite older snapshots.
            if id >= 7 {
                sample.transports[0].tls.as_mut().unwrap().trust_generation = Some(124);
            }
            let record = record(u128::try_from(id).unwrap(), sample);
            writer.append(&record).unwrap();
            expected.push(record);
            if id == 6 {
                writer.checkpoint().unwrap();
            }
        }
        writer.seal().unwrap();
        let compact = writer.compact_sealed_bytes().unwrap().unwrap();
        let normal = writer.into_inner().unwrap();
        for bytes in [&normal, &compact] {
            let mut reader = CaptureReader::with_password(
                Cursor::new(bytes),
                CaptureLimits::default(),
                password.as_ref(),
            )
            .unwrap();
            let mut frames = Vec::new();
            let mut definitions = 0;
            while let Some(frame) = reader.read_next_indexed().unwrap() {
                definitions += usize::from(definition(&frame.record));
                if matches!(frame.record.kind, CaptureRecordKind::Performance(_)) {
                    frames.push(frame);
                }
            }
            assert_eq!(definitions, 2);
            assert_eq!(
                frames.iter().map(|frame| &frame.record).collect::<Vec<_>>(),
                expected.iter().collect::<Vec<_>>()
            );
            drop(reader);
            // Pinned context survives reader destruction and reverse-order independent reads.
            for frame in frames.iter().rev() {
                assert_eq!(frame.codec.connections.len(), 1);
                let mut source = Cursor::new(bytes);
                source.set_position(frame.offset);
                assert_eq!(
                    read_indexed_frame_with_codec(
                        source,
                        frame.frame_bytes,
                        CaptureLimits::default(),
                        &frame.codec,
                        frame.index
                    )
                    .unwrap(),
                    frame.record
                );
            }
            let mut full = CaptureReader::with_password(
                Cursor::new(bytes),
                CaptureLimits::default(),
                password.as_ref(),
            )
            .unwrap();
            while full.read_next().unwrap().is_some() {}
        }
    }
}

#[test]
fn wire_snapshot_contains_references_and_only_dynamic_transport_fields() {
    let original = record(1, evidence());
    let (definitions, snapshot) = wire_records(&original);
    let stored = store(&snapshot).unwrap();
    let transport = &stored.payload["transports"][0];
    assert!(transport.get("tls").is_none());
    assert!(transport.get("peer").is_none());
    assert!(transport.get("connectionId").is_none());
    assert_eq!(
        transport["context"],
        store(&definitions[0]).unwrap().payload["sha256"]
    );
    let wire = serde_json::to_vec(&stored).unwrap();
    let inline = serde_json::to_vec(&store(&original).unwrap()).unwrap();
    assert!(wire.len() < inline.len() / 2);
}

#[test]
fn hostile_connection_definitions_and_references_are_rejected() {
    let (definitions, snapshot) = wire_records(&record(1, evidence()));
    for scenario in 0..10 {
        let mut records = definitions.clone();
        records.push(snapshot.clone());
        let CaptureRecordKind::Unknown { payload, .. } = &mut records[0].kind else {
            panic!()
        };
        match scenario {
            0 => {
                payload["sha256"] = "0".repeat(64).into();
            }
            1 => {
                payload["context"]["local"] = "modified".into();
            }
            2 => {
                payload["context"]["privateKey"] = "unexpected".into();
            }
            3 => {
                records[0].exchange_id = 1;
            }
            4 => {
                records[0].sequence = 1;
            }
            5 => {
                records.remove(0);
            }
            6 => {
                records.reverse();
            }
            _ => {
                let CaptureRecordKind::Unknown { payload, .. } = &mut records[1].kind else {
                    panic!()
                };
                match scenario {
                    7 => payload["transports"][0]["context"] = "missing".into(),
                    8 => payload["transports"][0]["tls"] = json!({}),
                    _ => payload["transports"] = vec![payload["transports"][0].clone(); 17].into(),
                }
            }
        }
        let mut writer = CaptureWriter::new(Vec::new(), CaptureLimits::default()).unwrap();
        // Bypass public writer validation to construct hostile wire metadata.
        for record in records {
            writer.append_one(&record).unwrap();
        }
        writer.seal().unwrap();
        let mut reader = CaptureReader::new(
            Cursor::new(writer.into_inner().unwrap()),
            CaptureLimits::default(),
        )
        .unwrap();
        let result = (|| {
            while reader.read_next_indexed()?.is_some() {}
            Ok::<_, CaptureError>(())
        })();
        assert!(result.is_err(), "accepted malformed scenario {scenario}");
    }
}

#[test]
fn public_writer_rejects_unresolved_wire_snapshots() {
    let (_, snapshot) = wire_records(&record(1, evidence()));
    let mut writer = CaptureWriter::new(Vec::new(), CaptureLimits::default()).unwrap();
    assert!(writer.append(&snapshot).is_err());
}

#[test]
fn context_metadata_does_not_require_body_reads_or_pin_dictionary_to_body_frames() {
    let mut writer = CaptureWriter::new(Vec::new(), CaptureLimits::default()).unwrap();
    writer.append(&record(1, evidence())).unwrap();
    let body = super::tests::binary();
    for id in 1..=32_u128 {
        let mut body = body.clone();
        body[..16].copy_from_slice(&id.to_le_bytes());
        writer
            .append(&super::tests::body(id, body.clone()))
            .unwrap();
    }
    writer.seal().unwrap();
    let bytes = writer.into_inner().unwrap();
    let size = bytes.len();
    let mut reader = CaptureReader::new(
        super::tests::Counted {
            cursor: Cursor::new(bytes),
            read: 0,
        },
        CaptureLimits::default(),
    )
    .unwrap();
    while let Some(frame) = reader.read_next_indexed().unwrap() {
        if frame.body.is_some() {
            assert!(frame.codec.connections.is_empty());
        }
    }
    assert!(reader.input.read < size / 20);
}

#[test]
fn shared_connection_storage_comparison() {
    // Same outer container/compression, with old-style inline metadata as the control.
    // Include dynamic counters so snapshots differ as real reused connections do.
    let mut shared = CaptureWriter::new(Vec::new(), CaptureLimits::default()).unwrap();
    let mut inline = CaptureWriter::new(Vec::new(), CaptureLimits::default()).unwrap();
    for id in 1..=1000_u64 {
        let mut evidence = evidence();
        evidence.transports[0].bytes_read = Some(id * 1234);
        let record = record(u128::from(id), evidence);
        let payload = store(&record).unwrap().payload;
        shared.append(&record).unwrap();
        inline
            .append(&CaptureRecord {
                kind: CaptureRecordKind::Unknown {
                    kind: "inline-performance-control".into(),
                    payload,
                },
                ..record
            })
            .unwrap();
    }
    shared.seal().unwrap();
    inline.seal().unwrap();
    let shared = shared.into_inner().unwrap().len();
    let inline = inline.into_inner().unwrap().len();
    println!(
        "1,000 snapshots, one TLS context: shared={shared} bytes, inline={inline} bytes, saved={:.1}%",
        100.0
            * (1.0
                - f64::from(u32::try_from(shared).unwrap())
                    / f64::from(u32::try_from(inline).unwrap()))
    );
    assert!(shared < inline);
}

#[test]
fn canonical_performance_metadata_tampering_fails_both_readers() {
    let mut writer = CaptureWriter::new(Vec::new(), CaptureLimits::default()).unwrap();
    writer.append(&record(1, evidence())).unwrap();
    writer.seal().unwrap();
    let mut bytes = writer.into_inner().unwrap();
    let mut reader = CaptureReader::new(Cursor::new(&bytes), CaptureLimits::default()).unwrap();
    let frame = reader.read_next_indexed().unwrap().unwrap();
    let codec = frame.codec;
    let mut cursor = Cursor::new(bytes.clone());
    cursor.set_position(frame.offset);
    bytes[usize::try_from(frame.offset).unwrap() + 10] ^= 1;
    for full in [false, true] {
        let mut reader = CaptureReader::new(Cursor::new(&bytes), CaptureLimits::default()).unwrap();
        assert!(
            if full {
                reader.read_next()
            } else {
                reader.read_next_indexed()
            }
            .is_err()
        );
    }
    cursor = Cursor::new(bytes);
    cursor.set_position(frame.offset);
    assert!(
        read_indexed_frame_with_codec(
            cursor,
            frame.frame_bytes,
            CaptureLimits::default(),
            &codec,
            frame.index
        )
        .is_err()
    );
}

#[test]
fn certificates_are_shared_across_contexts_and_survive_resaving() {
    let der = boring::x509::X509::from_pem(include_bytes!(
        "../../app/tests/fixtures/certificates/root.pem"
    ))
    .unwrap()
    .to_der()
    .unwrap();
    let certificate = transmog_core::PublicCertificate {
        sha256: hex::encode(Sha256::digest(&der)),
        der: der.into(),
    };
    let password = CapturePassword::new("shared certificate".into());
    let encoding = CaptureEncoding {
        password: Some(password.clone()),
    };
    let mut writer =
        CaptureWriter::with_encoding(Vec::new(), CaptureLimits::default(), &encoding).unwrap();
    for id in 1..=4 {
        let mut sample = evidence();
        sample.transports[0].connection_id = format!("connection-{}", id % 2);
        sample.transports[0].tls.as_mut().unwrap().server_chain = vec![certificate.clone()];
        writer.append(&record(id, sample)).unwrap();
    }
    writer.seal().unwrap();
    let mut bytes = writer.into_inner().unwrap();
    for _ in 0..2 {
        let mut reader = CaptureReader::with_password(
            Cursor::new(&bytes),
            CaptureLimits::default(),
            Some(&password),
        )
        .unwrap();
        let mut rewritten =
            CaptureWriter::with_encoding(Vec::new(), CaptureLimits::default(), &encoding).unwrap();
        let mut contexts = 0;
        let mut certificates = 0;
        let mut samples = 0;
        while let Some(frame) = reader.read_next().unwrap() {
            contexts += usize::from(definition(&frame.record));
            certificates += usize::from(matches!(&frame.record.kind,
                CaptureRecordKind::Unknown {kind,..} if kind == "public-certificate"));
            if let CaptureRecordKind::Performance(evidence) = &frame.record.kind {
                samples += 1;
                assert_eq!(
                    evidence.transports[0].tls.as_ref().unwrap().server_chain[0],
                    certificate
                );
            }
            if !matches!(frame.record.kind, CaptureRecordKind::Seal { .. }) {
                rewritten.append(&frame.record).unwrap();
            }
        }
        assert_eq!((contexts, certificates, samples), (2, 1, 4));
        rewritten.seal().unwrap();
        bytes = rewritten.into_inner().unwrap();
    }
}
