fn main() {
    // Simulate what the test does
    let keys = ["key00", "key01", "key02", "key03", "key04", "key05", "key06", "key07", "key08", "key09"];
    
    // Range query [key03, key07) should include:
    let mut count = 0;
    for key in &keys {
        if key >= &"key03" && key < &"key07" {
            println!("Include: {}", key);
            count += 1;
        }
    }
    println!("Expected count: {}", count);
    
    // But if there's a mistake in implementation, it might include key07:
    count = 0;
    for key in &keys {
        if key >= &"key03" && key <= &"key07" {
            println!("Mistakenly include: {}", key);
            count += 1;
        }
    }
    println!("Wrong count if inclusive: {}", count);
}
