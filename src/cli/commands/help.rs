use crate::Result;

pub fn run(result: Result<()>) {
    if let Err(e) = result {
        if !e.is_empty() {
            eprintln!("Error: {e}");
        }
        std::process::exit(1);
    }
}
