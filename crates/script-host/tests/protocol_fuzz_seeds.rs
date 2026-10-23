//! Ordinary replay ensures structured protocol seeds continue reaching typed parsers.

#[path = "../../../fuzz/support/script_protocol.rs"]
mod cases;

#[test]
fn protocol_semantic_fuzz_seeds_keep_framing_oracles_reachable() {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/script_protocol");
    let mut count = 0;
    for entry in std::fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            cases::check(&std::fs::read(entry.path()).unwrap());
            count += 1;
        }
    }
    assert!(count >= 8);
}
