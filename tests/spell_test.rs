//! Checks the spell checker against the system dictionary. Skipped silently
//! on platforms where the OS does not provide one.

use netpaste_app::spell;

#[test]
fn flags_misspellings_but_leaves_prose_and_pasted_text_alone() {
    if !spell::available() {
        return;
    }

    let text = "I beleive this is a tset of speling.";
    let bad: Vec<&str> = spell::check(text).into_iter().map(|r| &text[r]).collect();
    assert!(bad.contains(&"beleive"), "expected 'beleive' to be flagged, got {:?}", bad);
    assert!(bad.contains(&"tset"), "expected 'tset' to be flagged, got {:?}", bad);
    assert!(bad.contains(&"speling"), "expected 'speling' to be flagged, got {:?}", bad);
    for good in ["this", "is", "of"] {
        assert!(!bad.contains(&good), "'{}' should not be flagged, got {:?}", good, bad);
    }

    // The things people actually paste in here must not light up red.
    for quiet in [
        "ssh studio-admin@10.0.4.12 -p 2202",
        "https://example.com/some/path?q=xyzzy",
        "/Users/home/Library/Application Support/NetPaste",
        "sha256: 2f2f251380216ee2dc67bdead040a3da5b850e59b6db1dbc880c5eff9fb27e8a",
        "pass: EXAMPLE-not-a-real-key-9#x",
    ] {
        let flagged: Vec<&str> = spell::check(quiet).into_iter().map(|r| &quiet[r]).collect();
        assert!(flagged.is_empty(), "{:?} should be left alone, flagged {:?}", quiet, flagged);
    }
}

#[test]
fn offers_suggestions_for_a_misspelling() {
    if !spell::available() {
        return;
    }
    let s = spell::suggestions("speling");
    assert!(!s.is_empty(), "expected suggestions for 'speling'");
    assert!(
        s.iter().any(|w| w.eq_ignore_ascii_case("spelling")),
        "expected 'spelling' among {:?}",
        s
    );
}
