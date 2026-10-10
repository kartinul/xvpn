use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

static QUIET: AtomicBool = AtomicBool::new(false);

pub fn set_quiet(quiet: bool) {
    QUIET.store(quiet, Ordering::Relaxed);
}

pub fn is_quiet() -> bool {
    QUIET.load(Ordering::Relaxed)
}

pub fn kv(key: &str, value: &str) {
    if QUIET.load(Ordering::Relaxed) {
        return;
    }
    println!("{:<10} {}", key, value);
}

pub fn kv_prefix(prefix: &str, key: &str, value: &str) {
    if QUIET.load(Ordering::Relaxed) {
        return;
    }
    println!("{} {:<10} {}", prefix, key, value);
}

pub fn status(marker: &str, msg: &str) {
    if QUIET.load(Ordering::Relaxed) {
        return;
    }
    println!("{} {}", marker, msg);
}

pub fn section(title: &str) {
    if QUIET.load(Ordering::Relaxed) {
        return;
    }
    println!("{}", title);
}

pub fn list(items: &[&str]) {
    if QUIET.load(Ordering::Relaxed) {
        return;
    }
    if items.is_empty() {
        println!("  (none)");
    } else {
        for item in items {
            println!("  • {}", item);
        }
    }
}

pub fn table(rows: &[Vec<&str>], header: bool) {
    if QUIET.load(Ordering::Relaxed) {
        return;
    }
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
    if QUIET.load(Ordering::Relaxed) {
        return;
    }
    eprintln!("✗ {}", msg);
}

pub fn warn(msg: &str) {
    if QUIET.load(Ordering::Relaxed) {
        return;
    }
    eprintln!("~ {}", msg);
}

pub fn info(msg: &str) {
    if QUIET.load(Ordering::Relaxed) {
        return;
    }
    println!("→ {}", msg);
}

pub fn confirm(prompt: &str) -> bool {
    print!("{} [y/N]: ", prompt);
    io::stdout().flush().ok();
    let mut input = String::new();
    io::stdin().read_line(&mut input).ok();
    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}
