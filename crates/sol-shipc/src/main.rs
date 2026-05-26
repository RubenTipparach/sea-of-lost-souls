//! `sol-shipc` - the static-asset compiler CLI for Sea of Lost Souls.
//!
//! Subcommands:
//!   gen-test-ship                       Procedurally author the test
//!                                       interceptor's `model.glb`.
//!   build <ships_dir> --out <out_dir>   Validate + compile authored ships
//!                                       into runtime `.ship` + `.glb` artifacts.
//!
//! The `build` command is the gate: invalid colliders/nodes/power graphs make
//! it exit nonzero so CI fails on bad assets (see design.md §15).

mod build;
mod gen;
mod geometry;
mod glb;

use anyhow::{bail, Context};
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // Print the full error chain to stderr for a clear build-gate message.
            eprintln!("sol-shipc error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().context(USAGE)?;

    match cmd.as_str() {
        "gen-test-ship" => gen::run(),
        "build" => {
            let rest: Vec<String> = args.collect();
            let (ships_dir, out_dir) = parse_build_args(&rest)?;
            build::run(&ships_dir, &out_dir)
        }
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            Ok(())
        }
        other => bail!("unknown subcommand '{other}'\n\n{USAGE}"),
    }
}

/// Parse `build <ships_dir> --out <out_dir>` (flag may appear before or after
/// the positional dir).
fn parse_build_args(args: &[String]) -> anyhow::Result<(PathBuf, PathBuf)> {
    let mut ships_dir: Option<PathBuf> = None;
    let mut out_dir: Option<PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--out" | "-o" => {
                let v = args
                    .get(i + 1)
                    .context("--out requires a directory argument")?;
                out_dir = Some(PathBuf::from(v));
                i += 2;
            }
            positional => {
                if ships_dir.is_none() {
                    ships_dir = Some(PathBuf::from(positional));
                } else {
                    bail!("unexpected extra argument '{positional}'\n\n{USAGE}");
                }
                i += 1;
            }
        }
    }

    let ships_dir = ships_dir.context("missing <ships_dir>\n\n")?;
    let out_dir = out_dir.context("missing --out <out_dir>\n\n")?;
    Ok((ships_dir, out_dir))
}

const USAGE: &str = "\
sol-shipc - Sea of Lost Souls asset compiler

USAGE:
    sol-shipc gen-test-ship
    sol-shipc build <ships_dir> --out <out_dir>

EXAMPLES:
    sol-shipc gen-test-ship
    sol-shipc build assets/ships --out assets/compiled";
