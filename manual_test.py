# Test what our implementation should return
def generate_table():
    table = [0] * 256
    polynomial = 0xEDB88320
    
    for i in range(256):
        crc = i
        for _ in range(8):
            if crc & 1:
                crc = (crc >> 1) ^ polynomial
            else:
                crc >>= 1
        table[i] = crc & 0xFFFFFFFF
    
    return table

def slicing_by_8_tables(base_table):
    tables = [[0] * 256 for _ in range(8)]
    tables[0] = base_table[:]
    
    for i in range(256):
        crc = tables[0][i]
        for j in range(1, 8):
            crc = (crc >> 8) ^ tables[0][crc & 0xFF]
            tables[j][i] = crc & 0xFFFFFFFF
    
    return tables

def crc32_slicing_by_8(data):
    table = generate_table()
    tables8 = slicing_by_8_tables(table)
    
    crc = 0xFFFFFFFF
    offset = 0
    
    # Process 8 bytes at a time
    while offset + 8 <= len(data):
        bytes_chunk = data[offset:offset + 8]
        
        # XOR first 4 bytes into CRC
        crc ^= int.from_bytes(bytes_chunk[0:4], 'little')
        
        # Apply slicing-by-8
        crc = (tables8[7][(crc >> 24) & 0xFF] ^
               tables8[6][(crc >> 16) & 0xFF] ^
               tables8[5][(crc >> 8) & 0xFF] ^
               tables8[4][crc & 0xFF] ^
               tables8[3][bytes_chunk[4]] ^
               tables8[2][bytes_chunk[5]] ^
               tables8[1][bytes_chunk[6]] ^
               tables8[0][bytes_chunk[7]]) & 0xFFFFFFFF
        
        offset += 8
    
    # Process remaining bytes
    while offset < len(data):
        index = (crc ^ data[offset]) & 0xFF
        crc = ((crc >> 8) ^ table[index]) & 0xFFFFFFFF
        offset += 1
    
    return (~crc) & 0xFFFFFFFF

def crc32_simple(data):
    table = generate_table()
    crc = 0xFFFFFFFF
    
    for byte in data:
        index = (crc ^ byte) & 0xFF
        crc = ((crc >> 8) ^ table[index]) & 0xFFFFFFFF
    
    return (~crc) & 0xFFFFFFFF

# Test both implementations
test_data = b"123456789"
simple_result = crc32_simple(test_data)
slicing_result = crc32_slicing_by_8(test_data)

print(f"Simple CRC32: {simple_result} (0x{simple_result:08X})")
print(f"Slicing-by-8: {slicing_result} (0x{slicing_result:08X})")
print(f"Expected:     3421780262 (0xCBF43926)")
print(f"Simple matches: {simple_result == 3421780262}")
print(f"Slicing matches: {slicing_result == 3421780262}")
