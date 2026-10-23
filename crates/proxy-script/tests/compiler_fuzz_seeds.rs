//! Keep hand-maintained compiler seeds reaching their intended semantic oracles.

#[path = "../../../fuzz/support/script_compiler.rs"]
mod cases;

#[test]
fn semantic_compiler_seeds_replay_without_javascript_execution() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/script_compiler");
    let mut seeds = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    seeds.sort();
    assert!(
        seeds.len() >= 30,
        "Semantic compiler seeds were unexpectedly removed"
    );
    for seed in seeds {
        let bytes = std::fs::read(&seed).unwrap();
        assert!(!bytes.is_empty(), "{}", seed.display());
        cases::exercise(&bytes);
    }
}
