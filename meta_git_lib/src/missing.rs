use console::style;
use std::path::Path;

/// Print a unified, styled message for a missing (not cloned) repo.
pub fn print_missing_repo(_name: &str, url: &str, _path: &Path) {
    println!("  Repository is not cloned locally.");
    println!("  URL: {}", style(url).dim());
    println!(
        "  {}",
        style("→ Run `meta project update` to clone this repository.")
            .yellow()
            .bold()
    );
}
