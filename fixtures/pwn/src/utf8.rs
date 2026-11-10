use std::{
    borrow::Cow,
    io::{self, Read},
};

fn main() {
    let mut input = Vec::new();
    io::stdin().take(4096).read_to_end(&mut input).unwrap();
    let result = String::from_utf8_lossy(&input);
    println!(
        "{}",
        if matches!(result, Cow::Borrowed(_)) {
            "borrowed"
        } else {
            "owned"
        }
    );
    for byte in result.as_bytes() {
        print!("{byte:02x}");
    }
    println!();
}
