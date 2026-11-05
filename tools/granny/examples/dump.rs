//! Dev tool: dumps a gr2 for diffing against external decoders.
//!
//! `dump tree <file.gr2> [max_depth]` prints the object tree (floats as hex bits, first 4 array
//! elements). `dump sections <file.gr2> <out_dir>` writes each expanded section to
//! `<out_dir>/<stem>.s<i>.bin`. `dump crcs <file>` prints section CRCs of every gr2 embedded
//! in any file (e.g. a pack).

use std::fmt::Write as _;

use granny::{GrannyFile, Object, Value};

const MAX_ELEMENTS: usize = 4;

fn hex(b: &[u8]) -> String {
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

fn line(out: &mut String, indent: usize, text: &str) {
    let _ = writeln!(out, "{:width$}{text}", "", width = indent * 2);
}

fn value(out: &mut String, indent: usize, depth: usize, name: &str, v: Value<'_>) {
    let scalar = |s: String| format!("{name} = {s}");
    match v {
        Value::Inline(o) => {
            line(out, indent, &format!("{name}:"));
            object(out, indent + 1, depth, &o);
        }
        Value::Reference(r) | Value::VariantReference(r) => match r.get() {
            Ok(None) => line(out, indent, &scalar("null".into())),
            Ok(Some(o)) if depth > 0 => {
                line(out, indent, &format!("{name} ->"));
                object(out, indent + 1, depth - 1, &o);
            }
            Ok(Some(_)) => line(out, indent, &scalar("ref".into())),
            Err(e) => line(out, indent, &scalar(format!("error {e}"))),
        },
        Value::Array(a) => match a.resolve() {
            Ok(d) => {
                line(out, indent, &format!("{name} [{}]", d.len()));
                if depth > 0 {
                    for (i, o) in d.iter().take(MAX_ELEMENTS).enumerate() {
                        line(out, indent + 1, &format!("[{i}]"));
                        object(out, indent + 2, depth - 1, &o);
                    }
                }
            }
            Err(e) => line(out, indent, &scalar(format!("error {e}"))),
        },
        Value::ArrayOfReferences(a) => {
            let n = a.len().unwrap_or(0);
            line(out, indent, &format!("{name} [{n}]"));
            if depth > 0 {
                for i in 0..n.min(MAX_ELEMENTS) {
                    match a.get(i) {
                        Ok(Some(o)) => {
                            line(out, indent + 1, &format!("[{i}]"));
                            object(out, indent + 2, depth - 1, &o);
                        }
                        Ok(None) => line(out, indent + 1, &format!("[{i}] null")),
                        Err(e) => line(out, indent + 1, &format!("[{i}] error {e}")),
                    }
                }
            }
        }
        Value::String(s) => line(
            out,
            indent,
            &scalar(match s.to_string_lossy() {
                Ok(Some(s)) => format!("{s:?}"),
                Ok(None) => "null".into(),
                Err(e) => format!("error {e}"),
            }),
        ),
        Value::Transform(t) => {
            let mut b = t.flags.to_le_bytes().to_vec();
            for f in t
                .position
                .iter()
                .chain(&t.orientation)
                .chain(t.scale_shear.iter().flatten())
            {
                b.extend(f.to_le_bytes());
            }
            line(out, indent, &scalar(hex(&b)));
        }
        Value::Real32(f) => line(out, indent, &scalar(hex(&f.to_le_bytes()))),
        Value::Real16(h) => line(out, indent, &scalar(hex(&h.to_le_bytes()))),
        Value::EmptyReference => line(out, indent, &scalar("empty".into())),
        Value::Fixed(f) => {
            line(out, indent, &format!("{name} [{}]:", f.len()));
            for (i, e) in f.iter().enumerate() {
                value(out, indent + 1, depth, &format!("[{i}]"), e);
            }
        }
        other => match other.as_i64() {
            Some(i) => line(out, indent, &scalar(i.to_string())),
            None => line(out, indent, &scalar(other.kind_name().into())),
        },
    }
}

fn object(out: &mut String, indent: usize, depth: usize, o: &Object<'_>) {
    for (m, v) in o.fields() {
        value(out, indent, depth, &m.name, v);
    }
}

/// Prints `<offset> <section> <len> <crc32>` for every gr2 embedded in `data` (found by magic).
fn crcs(data: &[u8]) {
    let mut out = String::new();
    let mut i = 0;
    while let Some(k) = data
        .get(i..)
        .and_then(|d| d.windows(16).position(|w| w == granny::MAGIC_LE32))
    {
        let off = i + k;
        i = off + 16;
        match GrannyFile::parse(&data[off..]) {
            Ok(f) => {
                for s in 0..f.section_count() {
                    let b = f.section(s).unwrap_or(&[]);
                    let _ = writeln!(out, "{off} {s} {} {:08x}", b.len(), granny::crc32(b));
                }
            }
            Err(e) => {
                let _ = writeln!(out, "{off} error {e}");
            }
        }
    }
    print!("{out}");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let usage = "usage: dump tree <file> [depth] | dump sections <file> <out_dir>";
    let (Some(mode), Some(path)) = (args.get(1), args.get(2)) else {
        eprintln!("{usage}");
        std::process::exit(2);
    };
    let data = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    if mode == "crcs" {
        crcs(&data);
        return;
    }
    let file = match GrannyFile::parse(&data) {
        Ok(f) => f,
        Err(e) => {
            println!("error {e}");
            return;
        }
    };
    match mode.as_str() {
        "tree" => {
            let depth = args.get(3).and_then(|d| d.parse().ok()).unwrap_or(6);
            let mut out = String::new();
            match file.root() {
                Ok(root) => object(&mut out, 0, depth, &root),
                Err(e) => out.push_str(&format!("error {e}\n")),
            }
            print!("{out}");
        }
        "sections" => {
            let dir = std::path::Path::new(args.get(3).expect("out_dir"));
            let stem = std::path::Path::new(path)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            for i in 0..file.section_count() {
                let bytes = file.section(i).unwrap_or(&[]);
                std::fs::write(dir.join(format!("{stem}.s{i}.bin")), bytes).expect("write");
            }
        }
        _ => eprintln!("{usage}"),
    }
}
