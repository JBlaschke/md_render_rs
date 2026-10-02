```rust
use std::collections::HashMap;
use std::fmt::{self, Display};

/// Doc comment
#[derive(Debug, Clone, PartialEq)]
pub struct Point<'a, T: Copy> {
    pub x: T,
    name: &'a str,
}

impl<'a, T: Copy + Display> fmt::Display for Point<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {})", self.x, self.name)
    }
}

pub enum Shape { Circle(f64), Rect { w: u32, h: u32 } }

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut map: HashMap<String, i32> = HashMap::new();
    map.insert("a".to_string(), 1_000);
    let s = r#"raw "string""#;
    let c = b'x';
    match Some(3u8) {
        Some(n) if n > 2 => println!("{n}"),
        _ => unreachable!(),
    }
    for (k, v) in &map { let _ = (k, *v as f64 * 2.5e3); }
    unsafe { std::ptr::null::<u8>(); }
    Ok(())
}
```
