use clap::{Arg, Command, CommandFactory};
use clap_mangen::Man;
use std::fs;
use std::path::{Path, PathBuf};

fn main() -> std::io::Result<()> {
    let out_dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("man"));

    fs::create_dir_all(&out_dir)?;

    let cmd = keito_cli::cli::Cli::command();
    render_command_tree(&cmd, &out_dir, "keito")?;

    println!("Man pages generated in {}", out_dir.display());
    Ok(())
}

fn render_command_tree(cmd: &Command, out_dir: &Path, page_name: &str) -> std::io::Result<()> {
    let page_cmd = cmd.clone().name(page_name.to_string());
    let man = Man::new(page_cmd);
    let mut buf = Vec::new();
    man.render(&mut buf)?;
    fs::write(out_dir.join(format!("{page_name}.1")), buf)?;

    if cmd.has_subcommands() {
        render_help_page(out_dir, page_name)?;
    }

    for subcmd in cmd.get_subcommands() {
        let child_page_name = format!("{page_name}-{}", subcmd.get_name());
        render_command_tree(subcmd, out_dir, &child_page_name)?;
    }

    Ok(())
}

fn render_help_page(out_dir: &Path, parent_page_name: &str) -> std::io::Result<()> {
    let page_name = format!("{parent_page_name}-help");
    let help_cmd = Command::new(page_name.clone())
        .about("Print this message or the help of the given subcommand(s)")
        .arg(
            Arg::new("subcommands")
                .value_name("subcommands")
                .num_args(0..)
                .help("Command path to print help for"),
        );
    let man = Man::new(help_cmd);
    let mut buf = Vec::new();
    man.render(&mut buf)?;
    fs::write(out_dir.join(format!("{page_name}.1")), buf)
}
