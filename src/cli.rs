//! Command line parsing.

use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "mdroll",
    version,
    about = "A terminal Markdown viewer with big headings and real GFM support",
    after_help = "Environment: MDROLL_THEME, MDROLL_CONFIG, NO_COLOR\n\
                  Precedence: command line -> environment -> config file -> built-in default"
)]
pub struct Cli {
    /// Markdown file to view. Reads stdin when piped, browses *.md otherwise.
    pub file: Option<PathBuf>,

    /// Color theme. Default: `terminal`, which uses your terminal's own colors.
    #[arg(long, value_name = "NAME")]
    pub theme: Option<String>,

    /// Print available theme names and exit.
    #[arg(long)]
    pub list_themes: bool,

    /// Write a roff man page to stdout and exit.
    #[arg(long, hide = true)]
    pub man: bool,

    /// Start in reflow mode.
    #[arg(long, conflicts_with = "no_wrap")]
    pub wrap: bool,

    /// Start in horizontal-scroll mode.
    #[arg(long)]
    pub no_wrap: bool,

    /// Start in source view instead of rendered view.
    #[arg(long)]
    pub source: bool,

    /// Cap the reflow width. 0 means full terminal width.
    #[arg(long, value_name = "N")]
    pub width: Option<usize>,

    /// Blank columns to keep on each side. Default 2.
    #[arg(long, value_name = "N")]
    pub margin: Option<usize>,

    /// Show a persistent status line instead of transient toasts.
    #[arg(long, conflicts_with = "no_status")]
    pub status: bool,

    /// Use transient toasts instead of a persistent status line.
    #[arg(long)]
    pub no_status: bool,

    /// Enable mouse capture. Needed for image click actions.
    #[arg(long)]
    pub mouse: bool,

    /// Disable inline image rendering.
    #[arg(long)]
    pub no_images: bool,

    /// Never fetch images over the network; show their alt text instead.
    #[arg(long)]
    pub no_remote_images: bool,

    /// Whether the terminal speaks the Kitty graphics protocol: auto, kitty,
    /// or none. Detection reads variables the terminal sets, which do not
    /// survive `ssh`; `kitty` says to draw anyway.
    #[arg(long, value_name = "MODE")]
    pub graphics: Option<String>,

    /// Plain output, no ANSI styling.
    #[arg(long)]
    pub no_color: bool,

    /// How to draw mermaid blocks: auto, text, or image.
    #[arg(long, value_name = "MODE")]
    pub mermaid: Option<String>,

    /// Reload automatically when the file changes on disk.
    #[arg(long)]
    pub watch: bool,

    /// Never draw headings at double size, by either method.
    #[arg(short = 'z', long)]
    pub no_big_headings: bool,

    /// Treat East Asian Ambiguous characters as two columns wide.
    #[arg(long)]
    pub ambiguous_wide: bool,

    /// Use an alternate config file.
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn a_bare_filename_is_accepted() {
        let cli = Cli::parse_from(["mdroll", "README.md"]);
        assert_eq!(cli.file.unwrap().to_str().unwrap(), "README.md");
    }

    #[test]
    fn wrap_and_no_wrap_cannot_both_be_given() {
        assert!(Cli::try_parse_from(["mdroll", "--wrap", "--no-wrap"]).is_err());
    }

    #[test]
    fn flags_parse_together() {
        let cli = Cli::parse_from(["mdroll", "--theme", "nord", "--width", "80", "--source"]);
        assert_eq!(cli.theme.as_deref(), Some("nord"));
        assert_eq!(cli.width, Some(80));
        assert!(cli.source);
    }
}
