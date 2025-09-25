fn main() {
    // Test what the pattern "**/*.txt" should do
    let pattern = "**/*.txt";
    let test_case = "file.txt";
    
    println!("Pattern: {}", pattern);
    println!("Test: {}", test_case);
    
    // Expected behavior:
    // ** should match 0 or more path segments (directories)
    // / should match directory separator (but optional after **)
    // *.txt should match any file ending in .txt
    
    // So for "file.txt":
    // ** matches 0 directories (empty)
    // / should be optional/skipped since ** matched 0 directories  
    // *.txt matches "file.txt"
    
    println!("For '{}', ** should match 0 directories", test_case);
    println!("So the effective pattern becomes: *.txt");
    println!("Which should match: {}", test_case);
}
