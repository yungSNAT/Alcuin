use super::*;

/// Independent bit-by-bit CRC-32 (IEEE, reflected) reference used to check the table.
fn reference_table_entry(index: u32) -> u32 {
    let mut value = index;
    for _ in 0..8 {
        value = if value & 1 == 1 {
            (value >> 1) ^ 0xedb8_8320
        } else {
            value >> 1
        };
    }
    value
}

fn crc_with_table(table: &[u32; 256], bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in bytes {
        crc = table[((crc ^ u32::from(byte)) & 255) as usize] ^ (crc >> 8);
    }
    crc ^ u32::MAX
}

#[test]
fn crc_table_built_at_runtime_matches_the_ieee_reference_values() {
    let table = std::hint::black_box(crc_table());

    // Well-known anchor entries of the standard CRC-32 table.
    for (index, expected) in [
        (0usize, 0x0000_0000u32),
        (1, 0x7707_3096),
        (2, 0xee0e_612c),
        (8, 0x0edb_8832),
        (128, 0xedb8_8320),
        (255, 0x2d02_ef8d),
    ] {
        assert_eq!(table[index], expected, "table entry {index}");
    }

    // Every entry agrees with an independent bitwise computation.
    for (index, &value) in table.iter().enumerate() {
        assert_eq!(
            value,
            reference_table_entry(index as u32),
            "table entry {index}"
        );
    }

    // All entries are distinct, as expected of a CRC table.
    let unique: BTreeSet<u32> = table.iter().copied().collect();
    assert_eq!(unique.len(), 256);
}

#[test]
fn runtime_crc_table_produces_the_same_checksums_as_crc32_and_zip_headers() {
    let table = crc_table();
    for (input, expected) in [
        (&b""[..], 0x0000_0000u32),
        (&b"a"[..], 0xe8b7_be43),
        (&b"abc"[..], 0x3524_41c2),
        (&b"123456789"[..], 0xcbf4_3926),
        (
            &b"The quick brown fox jumps over the lazy dog"[..],
            0x414f_a339,
        ),
    ] {
        assert_eq!(crc_with_table(&table, input), expected, "{input:?}");
        assert_eq!(crc32(input), expected, "{input:?}");
    }

    // The CRC written into the local and central headers matches the runtime table.
    let payload = b"PUBLIC-browser-bundle-payload";
    let archive = stored_zip(&[ZipEntry {
        path: "PUBLIC/index.html",
        bytes: payload,
    }])
    .unwrap();
    let expected = crc_with_table(&table, payload).to_le_bytes();
    assert_eq!(&archive[14..18], &expected);

    let name_len = "PUBLIC/index.html".len();
    let central_start = 30 + name_len + payload.len();
    assert_eq!(
        &archive[central_start..central_start + 4],
        &0x0201_4b50u32.to_le_bytes()
    );
    assert_eq!(&archive[central_start + 16..central_start + 20], &expected);

    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&archive)).unwrap();
    let entry = zip.by_index(0).unwrap();
    assert_eq!(entry.crc32(), crc_with_table(&table, payload));
}
