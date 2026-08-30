// SPDX-License-Identifier: Apache-2.0
//! `zecor-gitengine` -- CLI wrapper. `--repo PATH` defaults to the current directory.
//!
//!   zecor-gitengine head
//!   zecor-gitengine rev-parse <REV>
//!   zecor-gitengine merge-base <A> <B>
//!   zecor-gitengine branch-state <BRANCH> --base <BASE>
//!   zecor-gitengine changed-files <A> <B>
//!   zecor-gitengine log [<REV>] [--limit N]
//!   zecor-gitengine status

use std::path::PathBuf;
use zecor_gitengine as ge;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let repo = flag(&args, "--repo")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    // positional args after the subcommand, dropping `--flag value` pairs
    let mut pos: Vec<String> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        if args[i].starts_with("--") {
            i += 2;
        } else {
            pos.push(args[i].clone());
            i += 1;
        }
    }

    let result: anyhow::Result<serde_json::Value> = match args.first().map(String::as_str) {
        Some("head") => ge::head(&repo).map(|h| serde_json::to_value(h).unwrap()),
        Some("rev-parse") => {
            ge::rev_parse(&repo, pos.first().map(|s| s.as_str()).unwrap_or("HEAD"))
                .map(|s| serde_json::json!({ "sha": s }))
        }
        Some("merge-base") => ge::merge_base(&repo, arg(&pos, 0), arg(&pos, 1))
            .map(|s| serde_json::json!({ "sha": s })),
        Some("branch-state") => {
            let base = flag(&args, "--base").unwrap_or_else(|| "main".into());
            ge::branch_state(&repo, arg(&pos, 0), &base).map(|s| serde_json::to_value(s).unwrap())
        }
        Some("changed-files") => ge::changed_files(&repo, arg(&pos, 0), arg(&pos, 1))
            .map(|v| serde_json::json!({ "files": v })),
        Some("log") => {
            let limit = flag(&args, "--limit")
                .and_then(|s| s.parse().ok())
                .unwrap_or(20);
            ge::log(
                &repo,
                pos.first().map(|s| s.as_str()).unwrap_or("HEAD"),
                limit,
            )
            .map(|v| serde_json::to_value(v).unwrap())
        }
        Some("status") => ge::status(&repo).map(|s| serde_json::to_value(s).unwrap()),
        _ => {
            eprintln!(
                "usage: zecor-gitengine <head | rev-parse REV | merge-base A B | \
                 branch-state BRANCH --base BASE | changed-files A B | log [REV] --limit N | \
                 status> [--repo PATH]"
            );
            std::process::exit(2);
        }
    };

    match result {
        Ok(v) => println!("{v}"),
        Err(e) => {
            eprintln!("zecor-gitengine: {e:#}");
            std::process::exit(1);
        }
    }
}

fn arg(pos: &[String], i: usize) -> &str {
    pos.get(i).map(|s| s.as_str()).unwrap_or_else(|| {
        eprintln!("zecor-gitengine: missing positional argument {}", i + 1);
        std::process::exit(2);
    })
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}
