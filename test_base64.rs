use std::fmt;

const STANDARD_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

const INVALID: u8 = 255;

#[derive(Debug, Clone, PartialEq)]
pub enum Base64Error {
    InvalidCharacter,
    InvalidLength,
}

impl fmt::Display for Base64Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Base64Error::InvalidCharacter => write!(f, "Invalid base64 character"),
            Base64Error::InvalidLength => write!(f, "Invalid base64 length"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Base64Config {
    alphabet: &'static [u8; 64],
    padding: bool,
    url_safe: bool,
}

impl Base64Config {
    pub fn standard() -> Self {
        Base64Config {
            alphabet: STANDARD_ALPHABET,
            padding: true,
            url_safe: false,
        }
    }

    fn create_decode_table(&self) -> [u8; 256] {
        let mut table = [INVALID; 256];
        for (i, &c) in self.alphabet.iter().enumerate() {
            table[c as usize] = i as u8;
        }
        // Handle padding
        table[b'=' as usize] = 64;
        table
    }
}

pub fn base64_decode_config(encoded: &str, config: Base64Config) -> Result<Vec<u8>, Base64Error> {
    if encoded.is_empty() {
        return Ok(Vec::new());
    }

    let decode_table = config.create_decode_table();
    let mut result = Vec::with_capacity((encoded.len() * 3) / 4);

    let bytes = encoded.as_bytes();
    let mut i = 0;

    // Process 4-character groups
    while i + 4 <= bytes.len() {
        let c1 = decode_table[bytes[i] as usize];
        let c2 = decode_table[bytes[i + 1] as usize];
        let c3 = if bytes[i + 2] == b'=' {
            64
        } else {
            decode_table[bytes[i + 2] as usize]
        };
        let c4 = if bytes[i + 3] == b'=' {
            64
        } else {
            decode_table[bytes[i + 3] as usize]
        };

        if c1 == INVALID || c2 == INVALID {
            return Err(Base64Error::InvalidCharacter);
        }

        result.push((c1 << 2) | (c2 >> 4));

        if c3 != 64 {
            if c3 == INVALID {
                return Err(Base64Error::InvalidCharacter);
            }
            result.push((c2 << 4) | (c3 >> 2));

            if c4 != 64 {
                if c4 == INVALID {
                    return Err(Base64Error::InvalidCharacter);
                }
                result.push((c3 << 6) | c4);
            }
        }

        i += 4;
    }

    // Handle remaining characters (for unpadded input)
    let remaining = bytes.len() - i;
    if remaining > 0 {
        if remaining == 1 {
            return Err(Base64Error::InvalidLength);
        }

        let c1 = decode_table[bytes[i] as usize];
        let c2 = decode_table[bytes[i + 1] as usize];

        if c1 == INVALID || c2 == INVALID {
            return Err(Base64Error::InvalidCharacter);
        }

        result.push((c1 << 2) | (c2 >> 4));

        if remaining == 3 {
            let c3 = decode_table[bytes[i + 2] as usize];
            if c3 == INVALID {
                return Err(Base64Error::InvalidCharacter);
            }
            result.push((c2 << 4) | (c3 >> 2));
        }
    }

    Ok(result)
}

pub fn base64_decode(encoded: &str) -> Result<Vec<u8>, Base64Error> {
    base64_decode_config(encoded, Base64Config::standard())
}

fn main() {
    let test_case = "SGVsbG8h!";
    println!("Testing: {}", test_case);
    
    let config = Base64Config::standard();
    let decode_table = config.create_decode_table();
    
    println!("Decode table for '!' (33): {}", decode_table[33]);
    println!("INVALID value: {}", INVALID);
    
    match base64_decode(test_case) {
        Ok(data) => println!("Decoded successfully: {:?}", data),
        Err(e) => println!("Error: {:?}", e),
    }
}
