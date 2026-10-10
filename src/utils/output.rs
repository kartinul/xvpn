use std::io::{self, Write};

pub fn kv(key: &str, value: &str) {
    println!("{:<10} {}", key, value);
}

pub fn kv_prefix(prefix: &str, key: &str, value: &str) {
    println!("{} {:<10} {}", prefix, key, value);
}

pub fn status(marker: &str, msg: &str) {
    println!("{} {}", marker, msg);
}

pub fn section(title: &str) {
    println!("{}", title);
}

pub fn list(items: &[&str]) {
    if items.is_empty() {
        println!("  (none)");
    } else {
        for item in items {
            println!("  • {}", item);
        }
    }
}

pub fn table(rows: &[Vec<&str>], header: bool) {
    if rows.is_empty() {
        return;
    }
    let cols = rows[0].len();
    let mut widths = vec![0; cols];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.len());
        }
    }
    for (ri, row) in rows.iter().enumerate() {
        let mut line = String::new();
        for (ci, cell) in row.iter().enumerate() {
            if ci > 0 {
                line.push_str("  ");
            }
            line.push_str(&format!("{:<width$}", cell, width = widths[ci]));
        }
        println!("{}", line);
        if header && ri == 0 {
            let mut sep = String::new();
            for (ci, w) in widths.iter().enumerate() {
                if ci > 0 {
                    sep.push_str("  ");
                }
                sep.push_str(&"─".repeat(*w));
            }
            println!("{}", sep);
        }
    }
}

pub fn error(msg: &str) {
    eprintln!("✗ {}", msg);
}

pub fn warn(msg: &str) {
    eprintln!("~ {}", msg);
}

pub fn info(msg: &str) {
    println!("→ {}", msg);
}

pub fn confirm(prompt: &str) -> bool {
    print!("{} [y/N]: ", prompt);
    io::stdout().flush().ok();
    let mut input = String::new();
    io::stdin().read_line(&mut input).ok();
    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}
