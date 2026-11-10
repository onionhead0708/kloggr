//! Generates a benchmark log file:
//! `cargo run --release -p kloggr-core --example genlog -- <typical|short|long|utf16> <MiB> <dir>`
//! Prints the path. Existing files are reused.

#[path = "../benches/support/generate.rs"]
mod generate;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [profile, size, dir] = args.as_slice() else {
        eprintln!("usage: genlog <typical|short|long|utf16> <MiB> <dir>");
        std::process::exit(2);
    };
    let profile = generate::Profile::parse(profile).expect("unknown profile");
    let size: u64 = size.parse().expect("size in MiB");
    println!(
        "{}",
        generate::cached(dir.as_ref(), profile, size).display()
    );
}
