//! Keep the incremental SSE fuzz oracle runnable on every development platform.
#[path = "../../../fuzz/support/sse_stream.rs"]
mod harness;

#[test]
fn semantic_seeds_preserve_events_across_chunking_and_content_codings() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/sse_stream");
    for entry in std::fs::read_dir(root).unwrap() {
        let input = std::fs::read(entry.unwrap().path()).unwrap();
        for flags in 0..16 {
            for chunk in [0, 1, 7, 255] {
                let mut input = input.clone();
                input[..2].copy_from_slice(&[flags, chunk]);
                harness::exercise(&input);
            }
        }
    }
}
