// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::ebx::Value as Ebx;
use crate::synthetic::{
    Asset, Db, Field, PartitionWriter, block, clear_header, write_installation,
};
use std::sync::atomic::{AtomicU32, Ordering};

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "nfs-frostbite-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn asset(name: &str, seed: u8, value: i32) -> Asset {
    Asset {
        name: name.into(),
        partition: PartitionWriter::default().build(
            [seed; 16],
            "Sample",
            [seed ^ 0xFF; 16],
            vec![("Value", Field::Int(value))],
        ),
    }
}

fn value(objects: &[crate::ebx::Object]) -> Option<&Ebx> {
    objects[0].fields.get("Value")
}

#[test]
fn base_installation_indexes_and_reads_assets() {
    let dir = TempDir::new();
    write_installation(
        &dir.0,
        b"exe",
        &[asset("a/one", 1, 10), asset("b/two", 2, 20)],
    )
    .unwrap();
    let install = Installation::open(&dir.0, Limits::default()).unwrap();
    assert_eq!(install.superbundles(), ["Win32/synthetic"]);
    assert_eq!(install.bundle_ids().unwrap(), ["win32/synthetic"]);
    let index = install.ebx_index().unwrap();
    assert_eq!(index.len(), 2);
    let location = &index["b/two"][0];
    assert_eq!(location.bundle, "win32/synthetic");
    let mut store = Store::open(&install).unwrap();
    let objects = store.objects(&location.entry).unwrap();
    assert_eq!(value(&objects), Some(&Ebx::Int32(20)));
}

#[test]
fn size_mismatch_missing_record_and_missing_layout_fail() {
    let dir = TempDir::new();
    write_installation(&dir.0, b"exe", &[asset("a/one", 1, 10)]).unwrap();
    let install = Installation::open(&dir.0, Limits::default()).unwrap();
    let mut entry = install.ebx_index().unwrap()["a/one"][0].entry.clone();
    let mut store = Store::open(&install).unwrap();
    entry.original_size += 1;
    assert!(matches!(store.bytes(&entry), Err(Error::Malformed(_))));
    entry.sha1 = [0xEE; 20];
    assert!(matches!(store.bytes(&entry), Err(Error::Missing(_))));
    std::fs::remove_file(dir.0.join("Data/cas_01.cas")).unwrap();
    let entry = install.ebx_index().unwrap()["a/one"][0].entry.clone();
    assert!(matches!(
        Store::open(&install).unwrap().bytes(&entry),
        Err(Error::Missing(_))
    ));
    std::fs::remove_file(dir.0.join("Data/layout.toc")).unwrap();
    assert!(matches!(
        Installation::open(&dir.0, Limits::default()),
        Err(Error::Missing(_))
    ));
}

#[test]
fn bounds_limit_bundles_and_entries() {
    let dir = TempDir::new();
    write_installation(
        &dir.0,
        b"exe",
        &[asset("a/one", 1, 10), asset("b/two", 2, 20)],
    )
    .unwrap();
    let limited = Limits {
        max_entries: 1,
        ..Limits::default()
    };
    let install = Installation::open(&dir.0, limited).unwrap();
    assert!(matches!(install.ebx_index(), Err(Error::Bound(_))));
    let tiny = Limits {
        max_container_bytes: 64,
        ..Limits::default()
    };
    assert!(matches!(
        Installation::open(&dir.0, tiny),
        Err(Error::Bound(_))
    ));
}

/// Add a patch layer: the patch TOC keeps the base bundle (`base` flag) and adds
/// a patch bundle whose entry is a delta of a base record plus a new record.
fn add_patch(root: &Path, base_record: [u8; 20], base_len: usize, replaced: &[u8]) {
    let data = root.join("Update/Patch/Data");
    std::fs::create_dir_all(data.join("Win32")).unwrap();
    // Delta: copy nothing from base, emit the replacement as delta blocks, skip base.
    let mut delta = ((3u32 << 28) | 1).to_be_bytes().to_vec();
    delta.extend(block(replaced, 9));
    delta.extend(((4u32 << 28) | 1).to_be_bytes());
    let fresh = asset("c/three", 3, 30);
    let fresh_record = block(&fresh.partition, 0);
    let mut cas = delta.clone();
    cas.extend(&fresh_record);
    std::fs::write(data.join("cas_01.cas"), &cas).unwrap();
    let mut catalog = b"NyanNyanNyanNyan".to_vec();
    for (sha, offset, size) in [
        ([0xD0; 20], 0usize, delta.len()),
        ([0xF0; 20], delta.len(), fresh_record.len()),
    ] {
        catalog.extend(sha);
        catalog.extend((offset as u32).to_le_bytes());
        catalog.extend((size as u32).to_le_bytes());
        catalog.extend(1u32.to_le_bytes());
    }
    std::fs::write(data.join("cas.cat"), &catalog).unwrap();
    let bundle = Db::Object(vec![(
        "ebx",
        Db::List(vec![
            Db::Object(vec![
                ("name", Db::str("a/one")),
                ("sha1", Db::Sha1([0xAA; 20])),
                ("originalSize", Db::Long(replaced.len() as i64)),
                ("casPatchType", Db::Int(2)),
                ("baseSha1", Db::Sha1(base_record)),
                ("deltaSha1", Db::Sha1([0xD0; 20])),
            ]),
            Db::Object(vec![
                ("name", Db::str("c/three")),
                ("sha1", Db::Sha1([0xF0; 20])),
                ("originalSize", Db::Long(fresh.partition.len() as i64)),
            ]),
        ]),
    )])
    .record(None);
    std::fs::write(data.join("Win32/synthetic.sb"), &bundle).unwrap();
    let toc = Db::Object(vec![(
        "bundles",
        Db::List(vec![
            Db::Object(vec![
                ("id", Db::str("win32/synthetic")),
                ("offset", Db::Long(0)),
                ("size", Db::Long(base_len as i64)),
                ("base", Db::Bool(true)),
            ]),
            Db::Object(vec![
                ("id", Db::str("win32/patched")),
                ("offset", Db::Long(0)),
                ("size", Db::Long(bundle.len() as i64)),
                ("delta", Db::Bool(true)),
            ]),
        ]),
    )]);
    std::fs::write(
        data.join("Win32/synthetic.toc"),
        clear_header(&toc.record(None)),
    )
    .unwrap();
}

#[test]
fn patch_layer_replaces_bundle_list_and_applies_deltas() {
    let dir = TempDir::new();
    write_installation(
        &dir.0,
        b"exe",
        &[asset("a/one", 1, 10), asset("b/two", 2, 20)],
    )
    .unwrap();
    let base_index = Installation::open(&dir.0, Limits::default())
        .unwrap()
        .ebx_index()
        .unwrap();
    let base_record = base_index["a/one"][0].entry.sha1;
    let base_len = std::fs::metadata(dir.0.join("Data/Win32/synthetic.sb"))
        .unwrap()
        .len() as usize;
    let replaced = asset("a/one", 1, 11).partition;
    add_patch(&dir.0, base_record, base_len, &replaced);
    let install = Installation::open(&dir.0, Limits::default()).unwrap();
    assert_eq!(
        install.bundle_ids().unwrap(),
        ["win32/synthetic", "win32/patched"]
    );
    let index = install.ebx_index().unwrap();
    // a/one is listed by the base bundle and, patched, by the patch bundle.
    assert_eq!(index["a/one"].len(), 2);
    let mut store = Store::open(&install).unwrap();
    let base = store.objects(&index["a/one"][0].entry).unwrap();
    assert_eq!(value(&base), Some(&Ebx::Int32(10)));
    let patched = store.objects(&index["a/one"][1].entry).unwrap();
    assert_eq!(value(&patched), Some(&Ebx::Int32(11)));
    let fresh = store.objects(&index["c/three"][0].entry).unwrap();
    assert_eq!(value(&fresh), Some(&Ebx::Int32(30)));
    let ranges = install.source_ranges().unwrap();
    // Two manifests and four distinct records. The base record shared by the
    // unpatched and delta entries is included only once.
    assert_eq!(ranges.len(), 6);
    for relative in [
        "Data/Win32/synthetic.sb",
        "Update/Patch/Data/Win32/synthetic.sb",
    ] {
        assert_eq!(
            ranges
                .iter()
                .filter(|r| r.path == dir.0.join(relative))
                .count(),
            1
        );
    }
    for relative in ["Data/cas_01.cas", "Update/Patch/Data/cas_01.cas"] {
        assert_eq!(
            ranges
                .iter()
                .filter(|r| r.path == dir.0.join(relative))
                .count(),
            2
        );
    }
    for source in ranges {
        assert!(
            source.offset + source.size as u64 <= std::fs::metadata(source.path).unwrap().len()
        );
    }
}

#[test]
fn non_cas_and_missing_superbundles_are_skipped() {
    let dir = TempDir::new();
    write_installation(&dir.0, b"exe", &[asset("a/one", 1, 10)]).unwrap();
    let layout = Db::Object(vec![(
        "superBundles",
        Db::List(vec![
            Db::Object(vec![("name", Db::str("Win32/synthetic"))]),
            Db::Object(vec![("name", Db::str("Win32/absent"))]),
            Db::Object(vec![("name", Db::str("Win32/chunks"))]),
        ]),
    )]);
    std::fs::write(
        dir.0.join("Data/layout.toc"),
        clear_header(&layout.record(None)),
    )
    .unwrap();
    let chunks = Db::Object(vec![("cas", Db::Bool(false))]);
    std::fs::write(
        dir.0.join("Data/Win32/chunks.toc"),
        clear_header(&chunks.record(None)),
    )
    .unwrap();
    let install = Installation::open(&dir.0, Limits::default()).unwrap();
    assert_eq!(install.superbundles().len(), 3);
    assert_eq!(install.ebx_index().unwrap().len(), 1);
    assert!(install.bundles("../escape").is_err());
}

#[test]
fn superbundle_ebx_lists_one_superbundle_with_first_listings() {
    use crate::synthetic::{CasSuperbundle, write_superbundles};
    let dir = TempDir::new();
    let first = [asset("items/car", 1, 1), asset("items/part", 2, 2)];
    let second = [asset("items/car", 3, 3), asset("other/thing", 4, 4)];
    write_superbundles(
        &dir.0,
        b"exe",
        &[
            CasSuperbundle {
                name: "Win32/levels/frontend/frontend",
                bundle: "win32/levels/frontend/frontend",
                assets: &first,
            },
            CasSuperbundle {
                name: "Win32/globals",
                bundle: "win32/default_settings_win32",
                assets: &second,
            },
        ],
        &[],
    )
    .unwrap();
    let install = Installation::open(&dir.0, Limits::default()).unwrap();
    let listed = install
        .superbundle_ebx("Win32/levels/frontend/frontend")
        .unwrap();
    let names: Vec<_> = listed.iter().map(|l| l.entry.name.as_str()).collect();
    assert_eq!(names, ["items/car", "items/part"]);
    let mut store = Store::open(&install).unwrap();
    assert_eq!(
        value(&store.objects(&listed[0].entry).unwrap()),
        Some(&Ebx::Int32(1))
    );
    assert_eq!(install.superbundle_ebx("Win32/globals").unwrap().len(), 2);
    assert!(install.superbundle_ebx("Win32/absent").unwrap().is_empty());
    let limits = Limits {
        max_entries: 1,
        ..Limits::default()
    };
    let small = Installation::open(&dir.0, limits).unwrap();
    assert!(small.superbundle_ebx("Win32/globals").is_err());
}

#[test]
fn inline_chunks_prefer_the_patch_layer_and_skip_cas_chunks() {
    use crate::synthetic::{InlineSuperbundle, write_superbundles};
    let dir = TempDir::new();
    let (a, b) = ([0xA1; 16], [0xB2; 16]);
    let base_chunks = [(a, b"base A".to_vec()), (b, vec![7u8; 70_000])];
    write_superbundles(
        &dir.0,
        b"exe",
        &[],
        &[InlineSuperbundle {
            name: "Win32/loc/en",
            chunks: &base_chunks,
        }],
    )
    .unwrap();
    let install = Installation::open(&dir.0, Limits::default()).unwrap();
    let chunks = install.inline_chunks("Win32/loc/en").unwrap();
    assert_eq!(chunks.len(), 2);
    assert_eq!(install.inline_chunk_bytes(&chunks[0]).unwrap(), b"base A");
    assert_eq!(
        install.inline_chunk_bytes(&chunks[1]).unwrap(),
        vec![7u8; 70_000],
        "multi-block chunk"
    );
    // A patch toc replaces chunk A and adds a cas-stored chunk, which is skipped.
    let patch = dir.0.join("Update/Patch/Data/Win32/loc");
    std::fs::create_dir_all(&patch).unwrap();
    let record = block(b"patched A", 9);
    std::fs::write(patch.join("en.sb"), &record).unwrap();
    let toc = Db::Object(vec![
        (
            "chunks",
            Db::List(vec![
                Db::Object(vec![
                    ("id", Db::Guid(a)),
                    ("offset", Db::Long(0)),
                    ("size", Db::Int(record.len() as i32)),
                ]),
                Db::Object(vec![
                    ("id", Db::Guid([0xC3; 16])),
                    ("sha1", Db::Sha1([1; 20])),
                ]),
            ]),
        ),
        ("cas", Db::Bool(false)),
    ]);
    std::fs::write(patch.join("en.toc"), clear_header(&toc.record(None))).unwrap();
    let chunks = install.inline_chunks("Win32/loc/en").unwrap();
    let ids: Vec<_> = chunks.iter().map(|c| c.id).collect();
    assert_eq!(ids, [a, b]);
    assert_eq!(
        install.inline_chunk_bytes(&chunks[0]).unwrap(),
        b"patched A"
    );
    assert!(install.inline_chunks("../escape").is_err());
    assert!(
        install
            .inline_chunks("Win32/loc/absent")
            .unwrap()
            .is_empty()
    );
    let limits = Limits {
        max_asset_bytes: 8,
        ..Limits::default()
    };
    let small = Installation::open(&dir.0, limits).unwrap();
    assert!(small.inline_chunks("Win32/loc/en").is_err(), "chunk bound");
    // A chunk range past the end of its .sb is malformed, not truncated data.
    let mut broken = chunks[1].clone();
    broken.source.offset += 1_000_000;
    assert!(install.inline_chunk_bytes(&broken).is_err());
}
