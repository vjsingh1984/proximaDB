fn generate_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let polynomial = 0xEDB88320u32;

    for i in 0..256 {
        let mut crc = i as u32;
        for _ in 0..8 {
            if crc & 1 == 1 {
                crc = (crc >> 1) ^ polynomial;
            } else {
                crc >>= 1;
            }
        }
        table[i] = crc;
    }

    table
}

fn crc32_test(data: &[u8]) -> u32 {
    let table = generate_table();
    let mut crc = 0xFFFFFFFF_u32;
    
    for &byte in data {
        let index = ((crc ^ byte as u32) & 0xFF) as usize;
        crc = (crc >> 8) ^ table[index];
    }
    
    !crc
}

fn main() {
    let test_cases: Vec<(&[u8], &str)> = vec![
        (b"", "empty"),
        (b"123456789", "standard"),
        (b"The quick brown fox jumps over the lazy dog", "fox"),
        (b"a", "single_a"),
        (b"abc", "abc"),
        (b"message digest", "message"),
    ];

    println!("Rust implementation CRC32 values:");
    for (data, name) in test_cases {
        let crc = crc32_test(data);
        println!("{}: {} (0x{:08X})", name, crc, crc);
    }
}
