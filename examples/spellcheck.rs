use netpaste_app::spell;
fn main() {
    for w in std::env::args().skip(1) {
        println!("{:<12} known={}  suggestions={:?}", w, spell::check(&w).is_empty(), spell::suggestions(&w));
    }
}
