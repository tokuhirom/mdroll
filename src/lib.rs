//! `mdroll` — a terminal Markdown viewer.
//!
//! The pipeline is a straight line, and each stage is a separate module:
//!
//! ```text
//! source text ──▶ parse ──▶ Vec<Block> ──▶ layout ──▶ Vec<Line> ──▶ render
//! ```
//!
//! [`layout::layout`] is a pure function of the blocks, the viewport, and the
//! mode. Wrap toggling, source toggling, and terminal resize are all handled by
//! throwing the layout away and recomputing it, so there is no incremental
//! state to drift out of sync.

pub mod app;
pub mod bigtext;
pub mod cache;
pub mod cli;
pub mod clipboard;
pub mod config;
pub mod fetch;
pub mod graphics;
pub mod highlight;
pub mod html;
pub mod ir;
pub mod keys;
pub mod layout;
pub mod mermaid;
pub mod mmdc;
pub mod parse;
pub mod render;
pub mod screen;
pub mod svg;
pub mod theme;
pub mod width;
pub mod wrap;
