use console::style;

/// Print a unified, styled message for a missing (not cloned) repo.
pub fn print_missing_repo(url: &str) {
    println!("  Repository is not cloned locally.");
    println!("  URL: {}", style(url).dim());
    println!(
        "  {}",
        style("→ Run `meta git update` to clone this repository.")
            .yellow()
            .bold()
    );
}
