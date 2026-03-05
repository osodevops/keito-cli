use clap::CommandFactory;
use clap_mangen::Man;
use std::fs;
use std::path::PathBuf;

fn main() -> std::io::Result<()> {
    let out_dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("man"));

    fs::create_dir_all(&out_dir)?;

    let cmd = keito_cli::cli::Cli::command();

    // Generate top-level man page
    let man = Man::new(cmd.clone());
    let mut buf = Vec::new();
    man.render(&mut buf)?;
    fs::write(out_dir.join("keito.1"), buf)?;

    // Generate subcommand man pages
    for subcmd in cmd.get_subcommands() {
        let name = format!("keito-{}", subcmd.get_name());
        let subcmd = subcmd.clone().name(name.clone());
        let man = Man::new(subcmd);
        let mut buf = Vec::new();
        man.render(&mut buf)?;
        fs::write(out_dir.join(format!("{name}.1")), buf)?;
    }

    println!("Man pages generated in {}", out_dir.display());
    Ok(())
}
