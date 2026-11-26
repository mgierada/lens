//! `lens` command-line entry point.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use globset::{Glob, GlobSet, GlobSetBuilder};

use lens::analysis::{AnalyzeOptions, analyze};
use lens::render::render_text;

/// Vendored or generated trees that are never worth indexing.
const DEFAULT_EXCLUDES: [&str; 6] = [
    "**/node_modules/**",
    "**/vendor/**",
    "**/site-packages/**",
    "**/.venv/**",
    "**/venv/**",
    "**/__pycache__/**",
];

#[derive(Parser)]
#[command(
    name = "lens",
    version,
    about = "Structure-aware branch comparison for code review"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Analyse changes of HEAD relative to BASE (merge-base semantics, like `BASE...HEAD`).
    Analyze(AnalyzeArgs),
}

#[derive(clap::Args)]
struct AnalyzeArgs {
    /// Base revision, or a range `BASE..HEAD` / `BASE...HEAD`.
    base: String,
    /// Head revision.
    #[arg(default_value = "HEAD")]
    head: String,
    /// Repository path.
    #[arg(long, default_value = ".")]
    repo: PathBuf,
    #[arg(long, value_enum, default_value_t = Format::Text)]
    format: Format,
    /// Pretty-print JSON output.
    #[arg(long)]
    pretty: bool,
    /// Context lines per hunk.
    #[arg(short = 'U', long, default_value_t = 3)]
    context: u32,
    /// Symbols with more distinct referrers than this do not merge clusters.
    #[arg(long, default_value_t = 30)]
    hub_threshold: usize,
    /// Impact entries kept per changed symbol.
    #[arg(long, default_value_t = 20)]
    impact_limit: usize,
    /// Extra glob of paths to skip (repeatable).
    #[arg(long)]
    exclude: Vec<String>,
    /// Skip files larger than this many bytes.
    #[arg(long, default_value_t = 1_000_000)]
    max_file_size: u64,
    /// Do not read or write the index cache in .git/lens.
    #[arg(long)]
    no_cache: bool,
    /// Skip per-hunk blame (faster on huge diffs).
    #[arg(long)]
    no_blame: bool,
    /// Text format: omit hunk contents.
    #[arg(long)]
    no_diff: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Text,
    Json,
    /// Interactive graph page (open in a browser).
    Html,
}

fn main() -> Result<()> {
    let Cli {
        command: Command::Analyze(args),
    } = Cli::parse();
    let (base, head) = split_range(&args.base, &args.head);
    let options = AnalyzeOptions {
        base,
        head,
        context: args.context,
        hub_threshold: args.hub_threshold,
        impact_limit: args.impact_limit,
        excludes: build_excludes(&args.exclude)?,
        max_file_size: args.max_file_size,
        use_cache: !args.no_cache,
        blame: !args.no_blame,
    };
    let report = analyze(&args.repo, &options)?;
    let output = match args.format {
        Format::Text => render_text(&report, !args.no_diff),
        Format::Json if args.pretty => serde_json::to_string_pretty(&report)?,
        Format::Json => serde_json::to_string(&report)?,
        Format::Html => lens::html::render_graph(&report, None)?,
    };
    let mut stdout = std::io::stdout().lock();
    match writeln!(stdout, "{output}") {
        // Output piped into e.g. `head` that exits early: not an error.
        Err(err) if err.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        result => Ok(result?),
    }
}

/// Accept `a..b` and `a...b` in the base argument; both use merge-base semantics.
fn split_range(base: &str, head: &str) -> (String, String) {
    let split = base.split_once("...").or_else(|| base.split_once(".."));
    match split {
        Some((b, h)) => (
            b.to_string(),
            if h.is_empty() { "HEAD" } else { h }.to_string(),
        ),
        None => (base.to_string(), head.to_string()),
    }
}

fn build_excludes(extra: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    let patterns = DEFAULT_EXCLUDES
        .iter()
        .copied()
        .chain(extra.iter().map(String::as_str));
    for pattern in patterns {
        builder.add(Glob::new(pattern).with_context(|| format!("bad glob '{pattern}'"))?);
    }
    Ok(builder.build()?)
}
