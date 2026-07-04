//! Throwaway corpus gate (delete after step 8): the store fixpoint over a real
//! inventory. Read-only — reconstructs and compares, never flushes.
//! Run: VIKNO_DIR=~/Projects/vikno cargo test --test store_corpus_spike -- --nocapture

use opys::project::Project;
use opys::store::Store;

#[test]
fn store_fixpoint_over_real_corpus() {
    let dir = std::env::var("VIKNO_DIR").unwrap_or_else(|_| {
        format!(
            "{}/Projects/vikno",
            std::env::var("HOME").unwrap_or_default()
        )
    });
    if !std::path::Path::new(&dir).join("opys.toml").exists() {
        eprintln!("SKIP: no opys.toml under {dir}");
        return;
    }
    let prj = Project::open(&dir).expect("open");
    let t0 = std::time::Instant::now();
    let (mut store, errs) = Store::open(&prj).expect("store open");
    let t_open = t0.elapsed();
    assert!(errs.is_empty(), "parse errors: {errs:?}");

    let (orig, _) = prj.load_docs();
    let t1 = std::time::Instant::now();
    let rebuilt = store.all_docs().expect("all_docs");
    let t_reconstruct = t1.elapsed();

    assert_eq!(orig.len(), rebuilt.len());
    let mut diffs = 0;
    for (o, (_, r)) in orig.iter().zip(&rebuilt) {
        if o.to_text() != r.to_text() || o.path != r.path {
            diffs += 1;
            if diffs <= 5 {
                eprintln!("FIXPOINT DIFF: {}", o.path.display());
            }
        }
    }
    eprintln!(
        "corpus: {} docs · open+materialize {:?} · reconstruct {:?} · diffs {}",
        orig.len(),
        t_open,
        t_reconstruct,
        diffs
    );
    assert_eq!(diffs, 0, "store round-trip changed documents");
}
