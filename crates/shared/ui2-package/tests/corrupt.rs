//! Damaged and malformed files: the reader answers with an error and never panics, whatever the
//! bytes. The base is the small package of `tests/common/mini.rs`.
#![cfg(feature = "write")]

use std::sync::Arc;

mod common;

use common::mini;
use common::raw::{self, ENTRY, HASH, HEADER};
use ui2_package::{Package, PackageError, SectionTag};

fn read(bytes: &[u8]) -> Result<Package, PackageError> {
    Package::read(Arc::from(bytes))
}

fn corrupt(at: Option<[u8; 4]>) -> PackageError {
    PackageError::Corrupt {
        at: at.map(SectionTag),
    }
}

const STRUCTURAL: [&[u8; 4]; 10] = [
    b"MANI", b"STRS", b"TMPL", b"STYL", b"STYP", b"TOKN", b"BIND", b"L10N", b"EXTN", b"SRCM",
];
const BLOBS: [&[u8; 4]; 3] = [b"FONT", b"IMGS", b"SCRP"];

fn tag(t: &[u8; 4]) -> String {
    SectionTag(*t).to_string()
}

#[test]
fn a_flipped_bit_in_any_section_breaks_the_hash() {
    let bytes = mini::build();
    for t in STRUCTURAL.iter().chain(&BLOBS).chain([&b"AREF"]) {
        let range = raw::range_of(&bytes, t);
        assert!(!range.is_empty(), "{}", tag(t));
        for i in range {
            for bit in 0..8 {
                let mut damaged = bytes.clone();
                damaged[i] ^= 1 << bit;
                assert_eq!(
                    read(&damaged).err(),
                    Some(corrupt(None)),
                    "{} byte {i} bit {bit}",
                    tag(t)
                );
            }
        }
    }
}

#[test]
fn a_flipped_bit_in_the_header_the_table_or_the_tail_is_refused() {
    let bytes = mini::build();
    let body = bytes.len() - HASH;
    for i in (0..HEADER + ENTRY * 14).chain(body..bytes.len()) {
        for bit in 0..8 {
            let mut damaged = bytes.clone();
            damaged[i] ^= 1 << bit;
            assert!(read(&damaged).is_err(), "byte {i} bit {bit}");
        }
    }
    // The same flips with a hash that was recomputed: the table checks stand on their own.
    for i in HEADER..HEADER + ENTRY * 13 {
        // The "reader must understand" bit of a section this reader knows changes nothing.
        let (entry, field) = ((i - HEADER) / ENTRY, (i - HEADER) % ENTRY);
        for bit in 0..8 {
            let mut damaged = bytes.clone();
            damaged[i] ^= 1 << bit;
            raw::rehash(&mut damaged);
            let at = HEADER + ENTRY * entry;
            let blob = BLOBS.iter().any(|t| t[..] == bytes[at..at + 4]);
            // A blob area may grow into its padding or shrink by a byte: it is any bytes.
            // The source map is the one optional section: renamed, it is an unknown optional one.
            let renamed_map = field < 4 && bytes[at..at + 4] == b"SRCM"[..];
            let harmless =
                (field == 4 && bit == 0) || (blob && (12..16).contains(&field)) || renamed_map;
            if let Ok(pkg) = read(&damaged) {
                assert!(
                    harmless,
                    "entry {entry} byte {field} bit {bit}: {:?}",
                    pkg.hash()
                );
            }
        }
    }
}

#[test]
fn flips_with_a_valid_hash_never_panic_and_mostly_fail() {
    let bytes = mini::build();
    let (mut total, mut refused) = (0, 0);
    for t in STRUCTURAL.iter().chain(&BLOBS).chain([&b"AREF"]) {
        for i in raw::range_of(&bytes, t) {
            for bit in [0u8, 3, 7] {
                let mut damaged = bytes.clone();
                damaged[i] ^= 1 << bit;
                raw::rehash(&mut damaged);
                // Bytes of a font, a script or a picture page are any bytes: they are not counted.
                let counted = STRUCTURAL.contains(t);
                total += usize::from(counted);
                match read(&damaged) {
                    Ok(pkg) => {
                        // A flip that lands on another valid value: the package is still usable.
                        let _ = pkg.verify_blobs();
                        let _ = pkg.scripts().count();
                    }
                    Err(
                        PackageError::Corrupt { .. }
                        | PackageError::Invalid { .. }
                        | PackageError::Incompatible { .. },
                    ) => refused += usize::from(counted),
                    Err(other) => panic!("{other:?}"),
                }
            }
        }
    }
    // Many flips land on another valid value (a letter of a name, a number); the rest break an index,
    // a length or a variant.
    assert!(refused * 4 > total, "{refused} of {total}");
}

#[test]
fn every_prefix_is_refused() {
    let bytes = mini::build();
    assert!(bytes.len() < 4096, "the base package stays small");
    for n in 0..bytes.len() {
        assert_eq!(read(&bytes[..n]).err(), Some(corrupt(None)), "prefix {n}");
    }
    // A prefix whose hash was made valid is a table that points outside the file, or a section
    // that ends too early.
    for n in (HEADER + HASH..bytes.len() - HASH).step_by(7) {
        let mut cut = bytes[..n].to_vec();
        cut.extend_from_slice(&[0; HASH]);
        raw::rehash(&mut cut);
        assert!(read(&cut).is_err(), "cut at {n}");
    }
}

#[test]
fn bytes_added_to_a_file_or_a_section_are_refused() {
    let bytes = mini::build();
    // After the hash.
    let mut longer = bytes.clone();
    longer.push(0);
    assert_eq!(read(&longer).err(), Some(corrupt(None)));
    // Between the last section and the hash, non-zero: the hash was recomputed.
    let mut longer = bytes[..bytes.len() - HASH].to_vec();
    longer.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    longer.extend_from_slice(&[0; HASH]);
    raw::rehash(&mut longer);
    assert_eq!(read(&longer).err(), Some(corrupt(None)));

    // Extra bytes at the end of a structural section's postcard value.
    for t in STRUCTURAL {
        let more = raw::with_payload(&bytes, t, |p| p.push(0));
        assert_eq!(read(&more).err(), Some(corrupt(Some(*t))), "{}", tag(t));
    }
    // Extra bytes inside a blob section's index leave the area shifted; a section with only
    // an area change is still one valid section when the index is untouched.
    for t in [b"FONT", b"IMGS"] {
        let more = raw::with_payload(&bytes, t, |p| {
            let index_len = u32::from_le_bytes([p[0], p[1], p[2], p[3]]) as usize;
            p.insert(4 + index_len, 0);
            p[..4].copy_from_slice(&(index_len as u32 + 1).to_le_bytes());
        });
        assert_eq!(read(&more).err(), Some(corrupt(Some(*t))), "{}", tag(t));
    }
    let more = raw::with_payload(&bytes, b"AREF", |p| p.push(0));
    assert_eq!(read(&more).err(), Some(corrupt(Some(*b"AREF"))));
}

#[test]
fn a_blob_index_longer_than_its_section_is_refused() {
    let bytes = mini::build();
    for t in BLOBS {
        let huge = raw::with_payload(&bytes, t, |p| {
            p[..4].copy_from_slice(&u32::MAX.to_le_bytes());
        });
        assert_eq!(read(&huge).err(), Some(corrupt(Some(*t))), "{}", tag(t));
        let empty = raw::with_payload(&bytes, t, Vec::clear);
        assert_eq!(read(&empty).err(), Some(corrupt(Some(*t))), "{}", tag(t));
    }
}

#[test]
fn overlapping_unaligned_and_out_of_file_sections_are_refused() {
    let bytes = mini::build();
    let entry = |i: usize| HEADER + ENTRY * i;
    let tag_of = |i: usize| {
        let at = entry(i);
        Some([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
    };
    let patch = |f: &dyn Fn(&mut Vec<u8>)| {
        let mut b = bytes.clone();
        f(&mut b);
        raw::rehash(&mut b);
        read(&b).err()
    };
    let put = |b: &mut Vec<u8>, at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    let get = |b: &[u8], at: usize| u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);

    // Section 1 starts where section 0 starts.
    let overlap = patch(&|b| {
        let first = get(b, entry(0) + 8);
        put(b, entry(1) + 8, first);
    });
    assert_eq!(overlap, Some(corrupt(tag_of(1))));
    // Not a multiple of 8.
    let unaligned = patch(&|b| {
        let off = get(b, entry(2) + 8);
        put(b, entry(2) + 8, off + 1);
    });
    assert_eq!(unaligned, Some(corrupt(tag_of(2))));
    // Longer than the file.
    let outside = patch(&|b| {
        let len = b.len() as u32;
        put(b, entry(3) + 12, len);
    });
    assert_eq!(outside, Some(corrupt(tag_of(3))));
    // Offset plus length wraps around a 32-bit number.
    let wrapping = patch(&|b| {
        put(b, entry(4) + 12, u32::MAX);
    });
    assert_eq!(wrapping, Some(corrupt(tag_of(4))));
    // An unknown flag bit.
    let flags = patch(&|b| put(b, entry(5) + 4, 2));
    assert_eq!(flags, Some(corrupt(tag_of(5))));
    // A gap that is not zero.
    let first_end = get(&bytes, entry(0) + 8) + get(&bytes, entry(0) + 12);
    if first_end % 8 != 0 {
        let gap = patch(&|b| b[first_end as usize] = 1);
        assert_eq!(gap, Some(corrupt(tag_of(1))));
    }
}

#[test]
fn tags_must_ascend_and_must_not_repeat() {
    let bytes = mini::build();
    let mut entries = raw::split(&bytes);
    entries.swap(0, 1);
    let swapped = read(&raw::assemble(&entries)).err();
    assert!(matches!(
        swapped,
        Some(PackageError::Corrupt { at: Some(_) })
    ));

    let mut entries = raw::split(&bytes);
    let again = entries[3].clone();
    entries.insert(4, again);
    let repeated = read(&raw::assemble(&entries)).err();
    assert!(matches!(
        repeated,
        Some(PackageError::Corrupt { at: Some(_) })
    ));
}

#[test]
fn a_missing_required_section_is_refused() {
    let bytes = mini::build();
    for t in [
        b"MANI", b"STRS", b"TMPL", b"STYL", b"TOKN", b"BIND", b"L10N", b"EXTN", b"FONT",
    ] {
        let mut entries = raw::split(&bytes);
        entries.retain(|e| &e.tag != t);
        assert_eq!(
            read(&raw::assemble(&entries)).err(),
            Some(corrupt(Some(*t))),
            "{}",
            tag(t)
        );
    }
    // The optional ones may go.
    let mut entries = raw::split(&bytes);
    entries.retain(|e| !matches!(&e.tag, b"SRCM" | b"AREF" | b"IMGS" | b"SCRP"));
    // Their users would dangle: the template refers to a picture and a reference.
    assert!(read(&raw::assemble(&entries)).is_err());
}

#[test]
fn not_a_package_at_all() {
    assert_eq!(read(&[]).err(), Some(corrupt(None)));
    assert_eq!(read(b"C5UI2PKG").err(), Some(corrupt(None)));
    let text = vec![b'a'; 1000];
    assert!(matches!(
        read(&text).err(),
        Some(PackageError::Incompatible { .. })
    ));
    // The right magic over zeros: the hash does not fit.
    let mut zeros = vec![0u8; 200];
    zeros[..8].copy_from_slice(b"C5UI2PKG");
    zeros[8] = 1;
    assert_eq!(read(&zeros).err(), Some(corrupt(None)));
}

/// A tiny deterministic generator for the mutation runs.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[test]
fn random_mutations_with_a_valid_hash_never_panic() {
    let base = mini::build();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut ok = 0;
    for round in 0..4000 {
        let mut b = base.clone();
        let body = b.len() - HASH;
        for _ in 0..=rng.below(4) {
            let at = rng.below(body);
            match rng.below(4) {
                0 => b[at] = rng.next() as u8,
                1 => b[at] ^= 1 << rng.below(8),
                2 => b[at] = b[at].wrapping_add(1),
                _ => {
                    let to = (at + 1 + rng.below(8)).min(body);
                    for x in &mut b[at..to] {
                        *x = 0xff;
                    }
                }
            }
        }
        raw::rehash(&mut b);
        if let Ok(pkg) = read(&b) {
            ok += 1;
            // What read must also be safe to use.
            let _ = pkg.verify_blobs();
            for f in pkg.fonts() {
                let _ = pkg.blob(f.blob);
            }
            let _ = pkg.scripts().count();
            let _ = pkg.source_loc(ui2_package::TemplateId(0), 0);
        }
        let _ = round;
    }
    assert!(ok < 4000);
}

#[test]
fn random_bytes_and_random_sections_never_panic() {
    let mut rng = Rng(42);
    // Bytes with no structure at all.
    for _ in 0..500 {
        let n = rng.below(300);
        let b: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
        assert!(read(&b).is_err());
    }
    // A valid container around random section payloads: only the decoders see the noise.
    let base = mini::build();
    for _ in 0..1500 {
        let mut entries = raw::split(&base);
        let at = rng.below(entries.len());
        let n = rng.below(64);
        entries[at].payload = (0..n).map(|_| rng.next() as u8).collect();
        if let Ok(pkg) = read(&raw::assemble(&entries)) {
            let _ = pkg.verify_blobs();
        }
    }
}

#[test]
fn a_damaged_zstd_blob_is_corrupt_in_its_section_when_used() {
    // The structure is fine; the compressed frame is not. Built with a stored frame header that
    // does not decode.
    use ui2_package::{BlobRef, BlobSection, Face, PackageBuilder};
    let raw_font = mini::bytes(5, 400);
    let mut b = PackageBuilder::new();
    let family = b.str("A");
    b.font(
        Face {
            family,
            weight: 400,
            italic: false,
            role: None,
            ranges: vec![],
            blob: BlobRef::unplaced(BlobSection::Font, true),
            blake3: *blake3::hash(&raw_font).as_bytes(),
        },
        &zstd::bulk::compress(&raw_font, 3).expect("zstd"),
        raw_font.len() as u32,
    );
    let bytes = b.finish(mini::manifest()).expect("finish");
    let packed = zstd::bulk::compress(&raw_font, 3).expect("zstd");
    let at = bytes
        .windows(16)
        .position(|w| w == &packed[..16])
        .expect("the frame is in the file");
    let mut damaged = bytes.clone();
    damaged[at + 8] ^= 0xff;
    damaged[at + 9] ^= 0x55;
    raw::rehash(&mut damaged);
    let pkg = read(&damaged).expect("the structure is fine");
    assert!(pkg.verify_blobs().is_err());
    let face = &pkg.fonts()[0];
    assert!(matches!(
        pkg.font_data(face),
        Err(PackageError::Corrupt { at: Some(t) }) if t == SectionTag::FONT
    ));
}
