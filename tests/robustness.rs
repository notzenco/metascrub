//! Files come from untrusted sources. Damaged input must produce an error
//! or a best-effort result, never a panic.

mod common;

const FIXTURES: [&str; 8] = [
    "photo.jpg",
    "photo.png",
    "photo.webp",
    "photo.heic",
    "report.pdf",
    "memo.docx",
    "budget.xlsx",
    "deck.pptx",
];

fn exercise(data: &[u8]) {
    let _ = metascrub::inspect(data);
    let _ = metascrub::clean(data);
}

#[test]
fn truncated_files_do_not_panic() {
    for name in FIXTURES {
        let data = common::fixture(name);
        for len in 0..data.len() {
            exercise(&data[..len]);
        }
    }
}

#[test]
fn corrupted_files_do_not_panic() {
    // A small deterministic generator keeps failures reproducible.
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for name in FIXTURES {
        let original = common::fixture(name);
        for _ in 0..300 {
            let mut data = original.clone();
            for _ in 0..1 + next() % 4 {
                let at = (next() as usize) % data.len();
                data[at] = next() as u8;
            }
            exercise(&data);
        }
    }
}
