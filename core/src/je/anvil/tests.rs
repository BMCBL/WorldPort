use super::*;
use crate::je::nbt;
use crate::nbt::{compound, tag_string};
use bedrock_nbt::NbtTag;

#[test]
fn region_round_trips_chunks() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("r.0.0.mca");
    let first = nbt::encode("", &compound([("Name", tag_string("first"))])).expect("encode");
    let second = nbt::encode("", &compound([("Name", tag_string("second"))])).expect("encode");
    write_region_file(
        &path,
        vec![(0, 0, first.clone()), (31, 31, second.clone())],
        Compression::default(),
    )
    .expect("write");

    let mut region = RegionFile::open(&path, false).expect("open");
    assert!(region.has_chunk(0, 0));
    assert!(region.has_chunk(31, 31));
    assert!(!region.has_chunk(1, 1));
    assert_eq!(
        region.read_chunk(0, 0).expect("read").as_deref(),
        Some(first.as_slice())
    );
    assert_eq!(
        region.read_chunk(31, 31).expect("read").as_deref(),
        Some(second.as_slice())
    );
}

#[test]
fn read_only_constructor_opens_a_write_protected_file() {
    let dir = std::env::temp_dir().join("worldport-region-readonly");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("r.0.0.mca");

    // Write a region normally, then drop write permission on the file.
    write_region_file(
        &path,
        vec![(
            0,
            0,
            crate::je::nbt::encode("", &compound([("xPos", bedrock_nbt::NbtTag::Int(0))]))
                .expect("encode"),
        )],
        Compression::new(1),
    )
    .expect("write region");

    let mut permissions = std::fs::metadata(&path).expect("metadata").permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&path, permissions).expect("chmod");

    // `open(.., false)` asks for write access and would fail here; the
    // read-only constructor must still open and read the chunk.
    let mut region = RegionFile::open_read_only(&path).expect("open read only");
    assert!(region.read_chunk(0, 0).expect("read").is_some());

    let mut permissions = std::fs::metadata(&path).expect("metadata").permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    let _ = std::fs::set_permissions(&path, permissions);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn zero_length_region_file_is_an_empty_region() {
    // Games leave 0-byte `.mca` files behind (an entities directory that
    // never received a chunk). That is not corruption.
    let dir = std::env::temp_dir().join("worldport-region-empty");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("r.0.0.mca");
    std::fs::write(&path, b"").expect("write empty file");

    let mut region = RegionFile::open_read_only(&path).expect("open empty region");
    assert!(region.read_chunk(0, 0).expect("read").is_none());
    assert!(region.occupied_slots().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn truncated_header_is_rejected() {
    let dir = std::env::temp_dir().join("worldport-region-short");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("r.0.0.mca");
    std::fs::write(&path, vec![0_u8; 100]).expect("write short file");

    let Err(error) = RegionFile::open_read_only(&path) else {
        panic!("a truncated header must be rejected");
    };
    assert!(error.to_string().contains("shorter than"), "{error}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn declared_length_beyond_the_allocated_sectors_is_rejected() {
    let dir = std::env::temp_dir().join("worldport-region-oversize");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("r.0.0.mca");
    write_region_file(&path, vec![(0, 0, vec![0_u8; 32])], Compression::new(1))
        .expect("write region");

    // The declared length excludes its own four-byte prefix.
    let mut bytes = std::fs::read(&path).expect("read file");
    let header = 4096_usize * 2;
    bytes[header..header + 4].copy_from_slice(&4093_u32.to_be_bytes());
    std::fs::write(&path, &bytes).expect("write file");

    let mut region = RegionFile::open_read_only(&path).expect("open");
    let error = region.read_chunk(0, 0).expect_err("must reject");
    assert!(error.to_string().contains("header allocates"), "{error}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn reads_an_external_lz4_chunk_at_negative_coordinates() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("r.-1.-2.mca");
    let payload = b"External chunk with a negative chunk position";
    let mut compressed = Vec::new();
    lz4_java_wrc::Lz4BlockOutput::new(&mut compressed)
        .write_all(payload)
        .unwrap();
    std::fs::write(dir.path().join("c.-1.-33.mcc"), compressed).unwrap();

    let mut region = vec![0; (HEADER_BYTES + SECTOR_BYTES) as usize];
    let slot = slot_index(31, 31) * 4;
    region[slot..slot + 4].copy_from_slice(&((2_u32 << 8) | 1).to_be_bytes());
    let offset = HEADER_BYTES as usize;
    region[offset..offset + 4].copy_from_slice(&1_u32.to_be_bytes());
    region[offset + 4] = EXTERNAL_CHUNK | 4;
    std::fs::write(&path, region).unwrap();

    let mut file = RegionFile::open_read_only(&path).unwrap();
    assert_eq!(
        file.read_chunk(31, 31).unwrap().as_deref(),
        Some(payload.as_slice())
    );
    assert!(file.read_chunk(32, 31).is_err());
}

#[test]
fn reads_gzip_payloads_from_mcregion_containers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("r.-1.0.mcr");
    let payload = nbt::encode(
        "",
        &compound([(
            "Level",
            compound([("Blocks", NbtTag::ByteArray(vec![1; 32768]))]),
        )]),
    )
    .unwrap();
    let mut region = RegionFile::open(&path, true).unwrap();
    region
        .write_chunk(
            31,
            0,
            &payload,
            RegionCompression::Gzip,
            Compression::fast(),
        )
        .unwrap();
    region.flush().unwrap();
    assert_eq!(region_coordinates(&path), Some((-1, 0)));
    let chunk = RegionFile::open_read_only(&path)
        .unwrap()
        .read_chunk(31, 0)
        .unwrap()
        .unwrap();
    assert_eq!(chunk, payload);
}

#[test]
fn region_append_write_and_delete() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("r.0.0.mca");
    let payload = nbt::encode("", &compound([("A", NbtTag::Int(7))])).expect("encode");
    let mut region = RegionFile::open(&path, true).expect("open");
    region
        .write_chunk(3, 4, &payload, RegionCompression::Zlib, Compression::fast())
        .expect("write");
    region.flush().expect("flush");
    drop(region);

    let mut region = RegionFile::open(&path, false).expect("reopen");
    assert_eq!(
        region.read_chunk(3, 4).expect("read").as_deref(),
        Some(payload.as_slice())
    );
    region.delete_chunk(3, 4);
    region.flush().expect("flush");
    assert!(!region.has_chunk(3, 4));
}
