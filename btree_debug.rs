fn main() {
    // Test the comparison logic
    let key06 = b"key06";
    let key07 = b"key07";
    let end = b"key07";
    
    println!("key06 >= key07: {}", key06 >= end);
    println!("key07 >= key07: {}", key07 >= end);
    
    // In exclusive range [start, end), key07 should NOT be included
    // So the condition should stop when key >= end
    // key06 < key07 so it should be included
    // key07 >= key07 so it should be excluded
}
